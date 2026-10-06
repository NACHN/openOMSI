//! The launcher's bus preview: the chosen bus drawn by the game's own renderer - its
//! model, paint and materials - on the plain (Vanilla+) path without the costly passes,
//! under the light of the chosen time and weather, into a picture the launcher shows
//! in a card. It is drawn again only when something changed (another bus, paint, light,
//! the preview turned by the mouse), never every frame.
//!
//! It shows a **scenery object** the same way - the same floor, the same light, the same
//! orbit, one object instead of a vehicle - which is what the editor's asset list has to
//! show and what a `.sco` name on a row cannot ([`Look::object`]). A bus is read on a worker
//! (its type, its scripts run to their resting state, its textures and meshes put on the
//! GPU ahead); an object is read where it is placed, because it goes into the scene itself
//! (`World::add_helper_object`) and the scene is the main thread's.
//!
//! Either way a fresh `World` is opened for it (whose caches belong to one scene); the old
//! scene goes when the new one is ready, so the picture never goes blank while switching.

use super::super::*;
use glam::DVec3;
use omsi_render::{Camera, Lighting, Renderer, Scene};
use std::sync::mpsc::{channel, Receiver};

/// What the showroom shows.
#[derive(Clone, PartialEq, Debug, Default)]
pub struct Look {
    pub root: PathBuf,
    /// The map the showroom's own `World` is opened on. A name, or a path under a content root
    /// (`maps/Grundorf/global.cfg`) - or a path of its own, which is what the editor hands over,
    /// since it has a map open already and knows exactly where it is.
    pub map: String,
    pub bus: String,
    pub paint: String,
    /// A scenery object (a `.sco`, spelled as a map's record spells it) to stand on the floor
    /// instead of a bus. A look with a bus and an object shows the bus.
    pub object: String,
    pub weather: String,
    /// Minutes of the day, and the date.
    pub time: i32,
    pub date: String,
}

/// What finished loading: the world it is shown in, and what stands on the floor.
struct Ready {
    look: Look,
    world: Arc<scene::World>,
    subject: Subject,
}

/// The thing on the floor.
enum Subject {
    /// A bus: its type, its scripts run to their resting state, and its meshes and textures
    /// put on the GPU ahead of the frame that shows it.
    Bus { vt: Arc<omsi_sim::VehicleType>, vehicle: omsi_sim::VehicleInstance, scheme: Option<usize> },
    /// A scenery object. Nothing is carried: it is read where it is placed, because placing it
    /// means putting meshes and instances into the scene, and only this thread has that.
    Object,
}

struct Shown {
    look: Look,
    scene: Scene,
    #[allow(dead_code)]
    world: Option<Arc<scene::World>>,
    vehicle: Option<omsi_sim::VehicleInstance>,
    render: Option<scene::VehicleRender>,
    trailers: Vec<scene::VehicleRender>,
    /// The scenery object on the floor, when the look is one: what it was put in with, so that
    /// it can be taken back out again.
    object: Option<scene::TileGpu>,
    /// Centre and size of what is on the floor (its bounding box), which decides where the
    /// camera stands and how far.
    centre: glam::Vec3,
    size: glam::Vec3,
    weather: omsi_content::weather::Weather,
    lighting: Lighting,
}

pub struct Showroom {
    shown: Option<Shown>,
    wanted: Option<Look>,
    /// The last look that could not be shown (not read again until something changes).
    failed: Option<Look>,
    loading: Option<(Look, Receiver<Result<Ready, String>>)>,
    pub error: Option<String>,
    /// Orbit: yaw and pitch (degrees) and distance factor, eased towards the targets.
    pub yaw: f32,
    pub pitch: f32,
    pub zoom: f32,
    yaw_to: f32,
    pitch_to: f32,
    zoom_to: f32,
    pub auto_turn: bool,
    idle: f32,
    /// Where the subject should appear on screen: the share of the width its centre is at.
    pub focus_x: f32,
    focus_now: f32,
    pub busy: bool,
    /// The picture: its texture and size, and whether it must be drawn again.
    target: Option<(wgpu::Texture, wgpu::TextureView, u32, u32)>,
    dirty: bool,
    /// Bumped whenever `target` was made anew (the interface binds it again).
    pub generation: u64,
}

fn args_for(look: &Look) -> Args {
    let mut v = vec!["omsi".to_string(), "--root".into(), look.root.to_string_lossy().to_string()];
    if !look.map.is_empty() {
        v.extend(["--map".into(), look.map.clone()]);
    }
    if !look.bus.is_empty() {
        v.extend(["--bus".into(), look.bus.clone()]);
    }
    if !look.paint.is_empty() {
        v.extend(["--paint".into(), look.paint.clone()]);
    }
    // (the current weather is fetched by the game, not by the preview)
    if !look.weather.is_empty() && !look.weather.starts_with("metar:") {
        v.extend(["--weather".into(), look.weather.clone()]);
    }
    v.extend(["--time".into(), format!("{:02}:{:02}", look.time / 60, look.time % 60)]);
    if !look.date.is_empty() {
        v.extend(["--date".into(), look.date.clone()]);
    }
    Args::try_parse_from(v).unwrap_or_else(|_| Args::parse_from(["omsi"]))
}

impl Showroom {
    pub fn new() -> Showroom {
        Showroom {
            shown: None,
            wanted: None,
            failed: None,
            loading: None,
            error: None,
            yaw: 215.0,
            pitch: 8.0,
            zoom: 1.0,
            yaw_to: 215.0,
            pitch_to: 8.0,
            zoom_to: 1.0,
            auto_turn: false,
            idle: 0.0,
            focus_x: 0.5,
            focus_now: 0.5,
            busy: false,
            target: None,
            dirty: true,
            generation: 0,
        }
    }

    /// Show this (a bus, its paint, the time and weather to light it with).
    pub fn want(&mut self, look: Look) {
        if self.wanted.as_ref() != Some(&look) {
            self.wanted = Some(look);
        }
    }

    /// The mouse dragged over the empty part of the window (degrees), or turned the wheel.
    pub fn orbit(&mut self, dx: f32, dy: f32) {
        self.yaw_to += dx * 0.35;
        self.pitch_to = (self.pitch_to + dy * 0.25).clamp(-4.0, 55.0);
        self.idle = 0.0;
    }
    pub fn zoom_by(&mut self, k: f32) {
        self.zoom_to = (self.zoom_to * k).clamp(0.55, 2.2);
        self.idle = 0.0;
    }

    /// Per frame: start loading what is wanted, take over what finished loading, move the
    /// camera. Returns true when a new scene was put in place.
    pub fn update(&mut self, renderer: &Renderer, dt: f32) -> bool {
        let mut swapped = false;
        // what finished loading
        if let Some((look, rx)) = self.loading.as_ref() {
            match rx.try_recv() {
                Ok(Ok(ready)) => {
                    self.loading = None;
                    // (cleared *before* placing it, because placing it can fail on its own terms -
                    // an object file that is not there, or one with nothing in it - and says so
                    // by setting this)
                    self.error = None;
                    self.shown = Some(self.place(renderer, ready));
                    self.dirty = true;
                    swapped = true;
                }
                Ok(Err(e)) => {
                    log::warn!("showroom {}: {e}", subject_name(look));
                    self.error = Some(e);
                    // (what was there before stayed in the picture as if it were the one chosen, and
                    // the failed one was read again every frame)
                    self.failed = Some(look.clone());
                    self.shown = None;
                    self.loading = None;
                }
                Err(std::sync::mpsc::TryRecvError::Empty) => {}
                Err(_) => {
                    self.failed = Some(look.clone());
                    self.loading = None;
                }
            }
        }
        // what is wanted and not shown or loading
        if let Some(w) = self.wanted.clone() {
            let shown = self.shown.as_ref().map(|s| &s.look);
            let loading = self.loading.as_ref().map(|l| &l.0);
            if shown != Some(&w) && loading != Some(&w) && self.loading.is_none() && self.failed.as_ref() != Some(&w) {
                // only the light changed: no need to read the bus (or the object) again
                let same_bus = shown.map(|s| s.bus == w.bus && s.paint == w.paint && s.map == w.map && s.root == w.root && s.object == w.object).unwrap_or(false);
                if same_bus {
                    if let Some(s) = self.shown.as_mut() {
                        s.look = w.clone();
                        let args = args_for(&w);
                        s.weather = load_weather(&args);
                        setup_sky(&args, renderer, &mut s.scene, omsi_content::Envir::load(&args.root.join("envir.cfg")).ok().as_ref(), Some(&s.weather));
                        s.lighting = lighting_for(&args, &s.weather);
                    }
                    self.dirty = true;
                } else {
                    self.start_loading(renderer, w);
                }
            }
        }
        self.busy = self.loading.is_some();
        // camera
        self.idle += dt;
        if self.auto_turn && self.idle > 4.0 {
            self.yaw_to += dt * 6.0;
        }
        let k = 1.0 - (-dt / 0.18).exp();
        // still turning: the picture must follow
        if (self.yaw_to - self.yaw).abs() > 0.05 || (self.pitch_to - self.pitch).abs() > 0.05 || (self.zoom_to - self.zoom).abs() > 0.001 {
            self.dirty = true;
        }
        self.yaw += (self.yaw_to - self.yaw) * k;
        self.pitch += (self.pitch_to - self.pitch) * k;
        self.zoom += (self.zoom_to - self.zoom) * k;
        self.focus_now += (self.focus_x - self.focus_now) * (1.0 - (-dt / 0.35).exp());
        // the bus's own state, whenever the picture is drawn again
        if let (true, Some(s)) = (self.dirty, self.shown.as_mut()) {
            if let (Some(v), Some(r)) = (s.vehicle.as_mut(), s.render.as_mut()) {
                player::sync_vehicle_transforms(renderer, &mut s.scene, v, r, &mut s.trailers, false);
            }
        }
        swapped
    }

    fn start_loading(&mut self, renderer: &Renderer, look: Look) {
        let args = args_for(&look);
        let root = look.root.clone();
        // a map named by a path of its own (the editor's, which it has open already) or one
        // looked up by name in the content roots
        let map_cfg = match Path::new(&look.map).is_absolute() {
            true => PathBuf::from(&look.map),
            false => omsi_cfg::resolve_path(&root, &look.map),
        };
        let date = start_clock(&args).date_code();
        let t0 = std::time::Instant::now();
        let world = match scene::World::open(&root, &map_cfg, date) {
            Ok(w) => {
                log::info!("showroom: {} opened in {:.2} s", map_cfg.display(), t0.elapsed().as_secs_f64());
                Arc::new(w)
            }
            Err(e) => {
                self.error = Some(format!("{e:#}"));
                self.wanted = None;
                return;
            }
        };
        let (tx, rx) = channel();
        let l2 = look.clone();
        // an object is placed on this thread and reads nothing ahead; a bus is the one that
        // wants a worker (its type, its scripts, its textures)
        let showing_an_object = !l2.object.is_empty() && l2.bus.is_empty();
        let prefetch = if showing_an_object { None } else { Some(world.vehicle_prefetch(renderer)) };
        std::thread::spawn(move || {
            let r = (|| -> Result<Ready> {
                if showing_an_object {
                    return Ok(Ready { look: l2, world, subject: Subject::Object });
                }
                if l2.bus.is_empty() {
                    return Err(anyhow!("no bus"));
                }
                let prefetch = prefetch.expect("a bus has one");
                let path = player_bus_path(&root, &l2.bus)?;
                let vt = Arc::new(omsi_sim::VehicleType::load(&root, &path)?);
                let scheme = paint_scheme(&vt, Some(l2.paint.as_str()).filter(|p| !p.is_empty()));
                let host = omsi_sim::VehicleHost::new(start_clock(&args));
                let mut vehicle = omsi_sim::VehicleInstance::new(vt.clone(), host);
                if let Some(i) = scheme {
                    for (var, v) in vt.paint_schemes[i].set_vars.clone() {
                        vehicle.set_var(&var, v);
                    }
                }
                load_coupled_parts(&root, &mut vehicle);
                for _ in 0..3 {
                    vehicle.update(1.0 / 30.0);
                }
                prefetch.prefetch(&vt, scheme);
                for t in &vehicle.trailers {
                    prefetch.prefetch(&t.ty, scheme.filter(|i| *i < t.ty.paint_schemes.len()));
                }
                Ok(Ready { look: l2, world, subject: Subject::Bus { vt, vehicle, scheme } })
            })();
            let _ = tx.send(r.map_err(|e| format!("{e:#}")));
        });
        self.loading = Some((look, rx));
    }

    fn place(&mut self, renderer: &Renderer, r: Ready) -> Shown {
        let t0 = std::time::Instant::now();
        let mut scene = renderer.new_scene();
        let args = args_for(&r.look);
        let weather = load_weather(&args);
        let envir = omsi_content::Envir::load(&args.root.join("envir.cfg")).ok();
        setup_sky(&args, renderer, &mut scene, envir.as_ref(), Some(&weather));
        add_floor(renderer, &mut scene);
        let world = r.world;
        let mut vehicle = None;
        let mut render = None;
        let mut trailers = Vec::new();
        let mut placed = None;
        // what the camera frames: the centre of what is on the floor, and how far it reaches
        let (centre, size, meshes) = match r.subject {
            Subject::Bus { vt, vehicle: mut v, scheme } => {
                v.position = DVec3::ZERO;
                v.heading = 0.0;
                let mesh_render = world.add_vehicle(renderer, &mut scene, &vt, scheme);
                let parts: Vec<scene::VehicleRender> = v.trailers.iter().map(|t| world.add_vehicle_part(renderer, &mut scene, &t.ty, scheme.filter(|i| *i < t.ty.paint_schemes.len()), &mesh_render)).collect();
                v.init_text_textures(&mut world.fonts.lock(), &|p| omsi_texture::decode_file(p).ok().map(|i| (i.width, i.height, i.rgba)));
                for t in v.trailers.iter_mut() {
                    t.init_text_textures(&mut world.fonts.lock(), &|p| omsi_texture::decode_file(p).ok().map(|i| (i.width, i.height, i.rgba)));
                }
                v.update(1.0 / 30.0);
                // the bus's size from its bounding box (with the rear section behind it)
                let bb = vt.def.bounding_box.unwrap_or([2.5, 12.0, 3.0, 0.0, 0.0, 1.5]);
                let mut length = bb[1];
                let mut centre = glam::Vec3::new(bb[3], bb[4], bb[5]);
                for t in &v.trailers {
                    let tb = t.ty.def.bounding_box.unwrap_or([2.5, 8.0, 3.0, 0.0, 0.0, 1.5]);
                    let back = (t.position - v.position).truncate().length() as f32 + tb[1] * 0.5;
                    let total = bb[1] * 0.5 + back;
                    centre.y = bb[4] + bb[1] * 0.5 - total * 0.5;
                    length = total;
                }
                let meshes = mesh_render.instances.len();
                vehicle = Some(v);
                render = Some(mesh_render);
                trailers = parts;
                // (the box the bus's own bounding box gives, with the length its trailers made)
                (centre, glam::Vec3::new(bb[0], length, bb[2]), meshes)
            }
            // an object: put it on the floor and measure what went in, so the camera frames it
            // rather than the bus-sized box it would otherwise be given
            Subject::Object => {
                let gpu = world.add_helper_object(renderer, &mut scene, &r.look.object, DVec3::ZERO, 0.0, &[]);
                let (centre, size) = gpu.as_ref().map(|g| box_of(&scene, &g.instances)).unwrap_or((glam::Vec3::ZERO, glam::Vec3::ZERO));
                let meshes = gpu.as_ref().map(|g| g.instances.len()).unwrap_or(0);
                if meshes == 0 {
                    // Nothing came of it: the file is not there, or nothing in it resolved. Said
                    // here rather than left to look like a load that never finishes - an editor's
                    // list can be pointed at a folder that has since gone.
                    self.error = Some(format!("{}: nothing to show", r.look.object));
                } else {
                    placed = gpu;
                }
                (centre, size, meshes)
            }
        };
        let lighting = lighting_for(&args, &weather);
        log::info!("showroom: {} ({} meshes, {:.1} x {:.1} x {:.1} m) placed in {:.2} s", subject_name(&r.look), meshes, size.x, size.y, size.z, t0.elapsed().as_secs_f64());
        Shown { look: r.look, scene, world: Some(world), vehicle, render, trailers, object: placed, centre, size, weather, lighting }
    }

    /// The picture of what is on the floor, at `w` x `h` pixels, drawn again when something
    /// changed.
    pub fn preview(&mut self, renderer: &mut Renderer, w: u32, h: u32) -> Option<wgpu::TextureView> {
        self.shown.as_ref()?;
        let (w, h) = (w.max(16), h.max(16));
        if self.target.as_ref().map(|t| (t.2, t.3) != (w, h)).unwrap_or(true) {
            let tex = renderer.device.create_texture(&wgpu::TextureDescriptor {
                label: Some("preview"),
                size: wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: renderer.format(),
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_SRC,
                view_formats: &[],
            });
            let view = tex.create_view(&Default::default());
            self.target = Some((tex, view, w, h));
            self.generation += 1;
            self.dirty = true;
        }
        if self.dirty {
            self.dirty = false;
            let view = self.target.as_ref().unwrap().1.clone();
            self.render(renderer, &view, w, h);
        }
        self.target.as_ref().map(|t| t.1.clone())
    }

    fn render(&mut self, renderer: &mut Renderer, target: &wgpu::TextureView, w: u32, h: u32) {
        let (yaw, pitch, zoom, focus) = (self.yaw, self.pitch, self.zoom, self.focus_now);
        let Some(s) = self.shown.as_mut() else { return };
        let fov = 30.0f32;
        let aspect = w as f32 / h.max(1) as f32;
        // far enough that all of it fits the free part of the window
        let half_v = (fov.to_radians() * 0.5).tan();
        // (the free part is the share of the window right of the panels: twice the room
        // from the focus to the right edge)
        let free = ((1.0 - focus) * 2.0).clamp(0.3, 1.0);
        // Its box's half-diagonal across the view against the free half-width, and its
        // half-height against the half-height: whichever needs more room. One number for both
        // would cut something off, and which one it is depends on the subject - a bus is long and
        // flat, a water tower is tall and narrow, and a showroom shows both.
        let across = (s.size.x * s.size.x + s.size.y * s.size.y).sqrt() * 0.5 + 1.0;
        let fit = (across / (half_v * aspect * free * 0.92).max(0.05)).max((s.size.z * 0.5 + 1.0) / (half_v * 0.92).max(0.05));
        // (the floor on the distance is a share of the subject's own size: eight metres, which
        // is right for a bus, would make a two-metre bench a thumbnail)
        let dist = (fit * zoom).max(s.size.length() * 0.35).max(1.2);
        let (sy, cy) = yaw.to_radians().sin_cos();
        let (sp, cp) = pitch.to_radians().sin_cos();
        let target_pt = DVec3::new(s.centre.x as f64, s.centre.y as f64, (s.centre.z * 0.75) as f64);
        // from the camera towards the bus: forward along the yaw, down by the pitch
        let dir = DVec3::new((sy * cp) as f64, (cy * cp) as f64, -sp as f64);
        let mut pos = target_pt - dir * dist as f64;
        pos.z = pos.z.max(0.6);
        // the subject stands in the free part of the window: the camera looks past it to the
        // side by as much
        let side = (focus - 0.5) * 2.0 * half_v * aspect;
        let look_yaw = yaw - side.atan().to_degrees();
        let cam = Camera { position: pos, yaw: look_yaw, pitch: -pitch, roll: 0.0, fov_deg: fov, near: 0.2, far: 6000.0 };
        s.scene.overlays.clear();
        let _ = &s.weather;
        renderer.render(&mut s.scene, target, w, h, &cam, &s.lighting);
    }

    /// Something is there to show - a bus, or a scenery object on the floor.
    pub fn has_picture(&self) -> bool {
        self.shown.as_ref().map(|s| s.vehicle.is_some() || s.object.is_some()).unwrap_or(false) && self.target.is_some()
    }

}

/// What a look is showing, for a log line: the bus, or the object, whichever it named.
fn subject_name(look: &Look) -> &str {
    if look.bus.is_empty() {
        &look.object
    } else {
        &look.bus
    }
}

/// The box a run of instances covers: its centre and its size, in the scene's own space.
///
/// Each mesh's own box through that instance's transform, over all eight corners - the object
/// and not the tile it was read with, because the instances handed back are the ones the object
/// was put in with (`World::add_helper_object`). A run that holds nothing is a point at the
/// origin, which is what an object OMSI could not resolve comes to.
fn box_of(scene: &Scene, instances: &[usize]) -> (glam::Vec3, glam::Vec3) {
    let (mut lo, mut hi) = (glam::Vec3::splat(f32::MAX), glam::Vec3::splat(f32::MIN));
    for &i in instances {
        let Some(inst) = scene.instances.get(i) else { continue };
        let Some(mesh) = scene.meshes.get(inst.mesh) else { continue };
        for k in 0..8 {
            let corner = glam::Vec3::new(
                if k & 1 == 0 { mesh.bounds_min.x } else { mesh.bounds_max.x },
                if k & 2 == 0 { mesh.bounds_min.y } else { mesh.bounds_max.y },
                if k & 4 == 0 { mesh.bounds_min.z } else { mesh.bounds_max.z },
            );
            let p = inst.transform.transform_point3(corner);
            lo = lo.min(p);
            hi = hi.max(p);
        }
    }
    if lo.x > hi.x {
        return (glam::Vec3::ZERO, glam::Vec3::ZERO);
    }
    ((lo + hi) * 0.5, hi - lo)
}

/// The light of the look's time and weather, with the sun's shadow under the bus. Always
/// the plain renderer, whatever the game's graphics setting: a preview is to be quick and
/// clear, not the game's picture (no enhanced exposure and glow, no weather effects).
fn lighting_for(args: &Args, weather: &omsi_content::weather::Weather) -> Lighting {
    let clock = start_clock(args);
    let envir = omsi_content::Envir::load(&args.root.join("envir.cfg")).ok();
    let daylight = omsi_sim::Daylight::compute(&clock, envir.as_ref());
    let mut l = weather_lighting(&daylight, weather, crate::weather_setup::cloud_drift_at(weather, clock.time), 0.0, true);
    l.shadows = daylight.altitude_deg > 2.0;
    l.enhanced = false;
    l.classic = false;
    l.detail = false;
    l.wetness = 0.0;
    l.snow = 0.0;
    l.fog_density = 0.0;
    l
}

/// A round showroom floor under the bus: dark, matt, catching its shadow.
fn add_floor(renderer: &Renderer, scene: &mut Scene) {
    let n = 96;
    let r = 400.0f32;
    let mut positions = vec![glam::Vec3::ZERO];
    let mut normals = vec![glam::Vec3::Z];
    let mut uvs = vec![glam::Vec2::ZERO];
    for k in 0..n {
        let a = std::f32::consts::TAU * k as f32 / n as f32;
        positions.push(glam::Vec3::new(a.cos() * r, a.sin() * r, 0.0));
        normals.push(glam::Vec3::Z);
        uvs.push(glam::Vec2::new(a.cos(), a.sin()));
    }
    let mut indices = Vec::new();
    for k in 0..n {
        indices.extend([0u32, 1 + ((k + 1) % n) as u32, 1 + k as u32]);
    }
    let data = omsi_geometry::MeshData { positions, normals, uvs, ranges: vec![(0, indices.len() as u32, 0)], indices, one_sided: false };
    let mesh = renderer.add_mesh(scene, &data);
    let mat = renderer.add_material(scene, None, omsi_render::AlphaMode::Opaque, [0.12, 0.125, 0.135, 1.0], false);
    renderer.add_instance(scene, mesh, DVec3::new(0.0, 0.0, -0.005), glam::Mat4::IDENTITY, vec![mat]);
}
