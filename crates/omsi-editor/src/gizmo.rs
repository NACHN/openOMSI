//! What is drawn on the chosen object: the bubble that says which one it is, and the gizmo
//! it is moved and turned by.
//!
//! The marks are ordinary instances of ordinary meshes, so they are lit, fogged and depth
//! tested by the same code that draws the map. Nothing here is a second drawing path.
//!
//! # Why this is not in the core
//!
//! `omsi-editor-core` is the map and what has been done to it; it has no camera and no
//! picture, and says so in its own header. A gizmo is nothing *but* those two - which handle
//! the pointer is over, how far a drag has come - so it lives here, above the core. The one
//! part of a drag that is a fact about the map rather than about the screen, a whole drag
//! becoming **one** step in the history, is in the core where it belongs
//! (`Session::begin_gesture`).
//!
//! # The axes are the map's, not the object's
//!
//! A map object has a heading and nothing else, so a gizmo drawn in the object's own frame
//! would have its east arrow pointing somewhere else on every object - while the readouts
//! beside it, the readouts a modder types in, are the map's. The arrows point east, north and
//! up whatever the object is turned to.
//!
//! # A gizmo is a screen size, not a map size
//!
//! The axes are as long as what it takes to come out `SPAN` pixels long from wherever the
//! camera is (see [`Gizmo::size_for`]), so it is the same handful of pixels to the pointer
//! whether the object is two metres or two hundred away - and it is the size it is *drawn*
//! at that a drag is measured with, or the two would disagree by a pixel or two, which is a
//! drag that goes the wrong way.

use crate::ui::Tool;
use glam::{DVec3, Mat4, Vec2, Vec3};
use omsi_editor_core::Selection;
use omsi_render::{AlphaMode, Camera, MeshData, MeshId, Renderer, Scene};

/// An axis a handle slides along: the map's own east, north and up.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Axis {
    East,
    North,
    Up,
}

impl Axis {
    pub const ALL: [Axis; 3] = [Axis::East, Axis::North, Axis::Up];

    /// Which way it goes in the world.
    pub fn dir(self) -> DVec3 {
        match self {
            Axis::East => DVec3::X,
            Axis::North => DVec3::Y,
            Axis::Up => DVec3::Z,
        }
    }

    /// What the status bar calls it.
    pub fn name(self) -> &'static str {
        match self {
            Axis::East => "east",
            Axis::North => "north",
            Axis::Up => "up",
        }
    }

    /// How the one arrow every axis is drawn with is turned into place: the mesh lies along
    /// +x, and a rotation about z turns a quarter turn for north, about y for up.
    fn turn(self) -> Mat4 {
        match self {
            Axis::East => Mat4::IDENTITY,
            Axis::North => Mat4::from_rotation_z(std::f32::consts::FRAC_PI_2),
            Axis::Up => Mat4::from_rotation_y(-std::f32::consts::FRAC_PI_2),
        }
    }

    /// The arrow's own colour: the three axes are told apart by it, as in every editor anyone
    /// has used.
    fn colour(self) -> [f32; 4] {
        match self {
            Axis::East => [0.86, 0.31, 0.27, 1.0],
            Axis::North => [0.44, 0.73, 0.36, 1.0],
            Axis::Up => [0.33, 0.56, 0.86, 1.0],
        }
    }
}

/// Which part of the gizmo the pointer is on.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Handle {
    /// One of the three arrows: drag along that axis.
    Slide(Axis),
    /// The ring: drag to turn the object.
    Turn,
}

impl Handle {
    /// How a status bar names it. Each of these is a whole phrase, because that is what the
    /// translation tables hold - `format!`-ing one out of parts finds no entry and comes out
    /// in English however the panels are set.
    pub fn name(self) -> String {
        match self {
            Handle::Slide(a) => format!("move {}", a.name()),
            Handle::Turn => "turning".to_string(),
        }
    }
}

/// Which gizmo is drawn, and so what a drag on it does.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum Kind {
    /// Nothing: the object is chosen but not being moved or turned.
    #[default]
    None,
    /// The three arrows.
    Slide,
    /// The ring round the object's foot.
    Turn,
}

impl Kind {
    /// The handles this gizmo is made of, which are the only ones on the screen.
    pub fn handles(self) -> &'static [Handle] {
        match self {
            Kind::None => &[],
            Kind::Slide => &[Handle::Slide(Axis::East), Handle::Slide(Axis::North), Handle::Slide(Axis::Up)],
            Kind::Turn => &[Handle::Turn],
        }
    }
}

/// Where the picture is and how big, as the gizmo needs it.
///
/// Built from the camera the frame is drawn with, at the origin the frame is drawn from, so
/// that a handle is where it looks like it is - which is the whole of what a gizmo has to get
/// right.
#[derive(Clone, Copy)]
pub struct Screen {
    proj: Mat4,
    origin: DVec3,
    width: f32,
    height: f32,
}

impl Screen {
    /// The camera's own projection: the same matrix the map is drawn with, so the screen and
    /// the picture cannot disagree. `origin` is the frame's render origin.
    pub fn new(camera: &Camera, origin: DVec3, width: u32, height: u32) -> Screen {
        let (w, h) = (width.max(1) as f32, height.max(1) as f32);
        Screen { proj: camera.view_proj(w / h, origin), origin, width: w, height: h }
    }

    /// Where a place in the world lands on the screen, in pixels. `None` behind the camera,
    /// which is a handle nobody can reach.
    pub fn project(&self, p: DVec3) -> Option<Vec2> {
        let clip = self.proj * (p - self.origin).as_vec3().extend(1.0);
        if clip.w <= 1e-6 {
            return None;
        }
        let ndc = clip.truncate() / clip.w;
        Some(Vec2::new((ndc.x * 0.5 + 0.5) * self.width, (0.5 - ndc.y * 0.5) * self.height))
    }
}

/// How far from the pointer a handle is picked up, in pixels.
pub const REACH: f32 = 9.0;
/// How long the axes come out on the screen, in pixels.
pub const SPAN: f32 = 92.0;
/// How much longer the handle under the pointer is drawn - the whole of the hover: there is
/// no way to recolour one slot of a drawn instance, and a handle that grows is what a hand
/// expects anyway. Picking measures the same length, so a handle that grew stays held.
const HOT: f64 = 1.12;

/// The gizmo as it stands on the map: where it is, and how long its axes are in metres.
#[derive(Clone, Copy, Debug)]
pub struct Gizmo {
    /// The object's foot: where an arrow starts and the ring is centred.
    pub at: DVec3,
    /// One axis's length in metres - everything on the gizmo scales with it.
    pub size: f64,
}

impl Gizmo {
    /// How long the axes have to be to come out [`SPAN`] pixels long from `distance` metres
    /// away through a picture `height` pixels tall and a lens `fov` degrees wide.
    ///
    /// The field of view is vertical, so at one metre a metre spans `height / 2·tan(fov/2)`
    /// pixels. Clamped at both ends: a gizmo smaller than an object's thumb has no handle to
    /// grab, and one longer than a street is in the way.
    pub fn size_for(distance: f64, height: f32, fov_deg: f32, span: f32) -> f64 {
        let per_metre = height as f64 / (2.0 * (fov_deg as f64 * 0.5).to_radians().tan()).max(1e-3);
        (distance * span as f64 / per_metre.max(1e-3)).clamp(0.05, 500.0)
    }

    /// How long a handle is drawn when `hot` is the one the pointer is on.
    fn drawn(&self, handle: Handle, hot: Option<Handle>) -> f64 {
        self.size * if hot == Some(handle) { HOT } else { 1.0 }
    }

    /// Where a handle is in the world at `t` along it: `t` in 0..1 for an arrow, 0..1 round
    /// the ring.
    fn point(&self, handle: Handle, t: f64, hot: Option<Handle>) -> DVec3 {
        let size = self.drawn(handle, hot);
        match handle {
            Handle::Slide(a) => self.at + a.dir() * (size * t),
            Handle::Turn => {
                let (s, c) = (t * std::f64::consts::TAU).sin_cos();
                self.at + DVec3::new(size * s, size * c, 0.0)
            }
        }
    }

    /// The ring is drawn and picked as this many straight pieces.
    const RING_STEPS: usize = 48;

    /// What the pointer at `pointer` (pixels) is on, if anything.
    ///
    /// Handles are tested as what they are drawn as - a line for an arrow, a chain of short
    /// lines for the ring - so what is grabbed is what is under the pointer. Only the handles
    /// of `kind` are looked at, because only those are on the screen: the arrows and the ring
    /// both reach the object's own length, so a ring that is not drawn would otherwise be
    /// picked up under every arrow's tip.
    ///
    /// `hot` is last frame's answer, passed back in because the handle under the pointer is
    /// drawn longer (see [`HOT`]) and the pick has to measure the same length or the tip
    /// hovers on and off. A handle pointing straight at the camera is no line at all and
    /// cannot be grabbed; the arrow keys still move that axis.
    pub fn pick(&self, screen: &Screen, kind: Kind, pointer: Vec2, hot: Option<Handle>) -> Option<Handle> {
        let mut best: Option<(f32, Handle)> = None;
        for handle in kind.handles().iter().copied() {
            let Some(d) = self.near(screen, pointer, handle, hot) else { continue };
            if d <= REACH && best.map_or(true, |(bd, _)| d < bd) {
                best = Some((d, handle));
            }
        }
        best.map(|(_, h)| h)
    }

    /// How far the pointer is from a handle, in pixels - `None` when the handle is behind the
    /// camera.
    fn near(&self, screen: &Screen, pointer: Vec2, handle: Handle, hot: Option<Handle>) -> Option<f32> {
        let steps = match handle {
            Handle::Slide(_) => 4,
            Handle::Turn => Self::RING_STEPS,
        };
        let mut pts: Vec<Vec2> = Vec::with_capacity(steps + 1);
        for k in 0..=steps {
            pts.push(screen.project(self.point(handle, k as f64 / steps as f64, hot))?);
        }
        let mut best = f32::MAX;
        for w in pts.windows(2) {
            best = best.min(distance_to_segment(pointer, w[0], w[1]));
        }
        Some(best)
    }

    /// How far along `axis` the pointer has come since it was grabbed, in metres.
    ///
    /// The axis is a line on the screen; how far the pointer moved along that line, scaled by
    /// how many metres a pixel of it is worth, is how far the object has been dragged. No ray
    /// is cast and no plane is intersected: an axis nearly parallel to the view has no usable
    /// intersection at all, and what a handle under the pointer means is this.
    ///
    /// `None` when the axis has shrunk to a couple of pixels - it points at the camera, and
    /// the caller leaves the object where it is rather than flinging it across the map.
    pub fn slide(&self, screen: &Screen, axis: Axis, grab: Vec2, now: Vec2) -> Option<f64> {
        let p0 = screen.project(self.at)?;
        let p1 = screen.project(self.at + axis.dir() * self.size)?;
        let d = p1 - p0;
        let len = d.length();
        if len < 6.0 {
            return None;
        }
        Some(((now - grab).dot(d / len) as f64) * (self.size / len as f64))
    }
}

/// The turn the pointer asks for, in degrees: the direction from the object to the place the
/// drag started on, against the direction to where it is now.
///
/// Both places lie on the horizontal plane through the object, so this is the heading being
/// turned and nothing else. The result is the short way round, so a drag past the far side of
/// the object does not spin it a whole turn.
pub fn turn(at: DVec3, from: DVec3, to: DVec3) -> f64 {
    let angle = |p: DVec3| (p.x - at.x).atan2(p.y - at.y).to_degrees();
    (angle(to) - angle(from) + 180.0).rem_euclid(360.0) - 180.0
}

/// How far a point is from a line segment, in the plane of the screen.
fn distance_to_segment(p: Vec2, a: Vec2, b: Vec2) -> f32 {
    let ab = b - a;
    let len2 = ab.length_squared();
    if len2 < 1e-6 {
        return (p - a).length();
    }
    let t = ((p - a).dot(ab) / len2).clamp(0.0, 1.0);
    (p - (a + ab * t)).length()
}

/// The sphere round everything a set of instances draws: the union of their bounding spheres.
///
/// A sphere and not a box, because the renderer keeps one bounding sphere per mesh - it is what
/// the culling and level-of-detail tests ask for - and a bubble that says *this one, here* is
/// the whole job of a selection mark. A long object therefore gets a bubble wider than itself,
/// which is honest enough: it is a mark, and the readouts in the panel are the measurement.
pub fn bubble(scene: &Scene, instances: &[usize]) -> Option<(DVec3, f64)> {
    let mut out: Option<(DVec3, f64)> = None;
    for inst in instances {
        let Some(i) = scene.instances.get(*inst) else { continue };
        let Some(mesh) = scene.meshes.get(i.mesh) else { continue };
        if !(mesh.bounds_radius > 0.0) {
            continue;
        }
        // the mesh's bounding sphere is in the object's own frame; the instance's transform
        // is what puts it on the map (see `Renderer::instance_entries`)
        let (scale, _, _) = i.transform.to_scale_rotation_translation();
        let centre = i.origin + (i.transform * mesh.bounds_center.extend(1.0)).truncate().as_dvec3();
        let radius = mesh.bounds_radius as f64 * scale.abs().max_element().max(1e-6) as f64;
        out = Some(match out {
            None => (centre, radius),
            Some((c, r)) => {
                let d = (centre - c).length();
                if radius > r {
                    (centre, radius.max(d + r))
                } else {
                    (c, r.max(d + radius))
                }
            }
        });
    }
    out
}

// ---- what is drawn -----------------------------------------------------------------------

/// What the frame wants drawn over the map.
#[derive(Clone, Copy, Debug, Default)]
pub struct Show {
    /// The bubble round the chosen object: where its middle is, and how big.
    pub bubble: Option<(DVec3, f64)>,
    /// The gizmo: where it stands, and its axis length in metres.
    pub gizmo: Option<(DVec3, f64)>,
    /// Which gizmo that is.
    pub kind: Kind,
    /// Which part of it the pointer is over.
    pub hot: Option<Handle>,
}

impl Show {
    /// The gizmo as it is placed for this frame.
    pub fn gizmo(&self) -> Option<Gizmo> {
        self.gizmo.map(|(at, size)| Gizmo { at, size })
    }
}

/// The gizmo a tool puts in hand: the arrows for a move, the ring for a turn, and nothing for
/// the tools that move nothing - arrows over an object nobody is moving are in the way of the
/// map.
pub fn kind_for(tool: Tool) -> Kind {
    match tool {
        Tool::Move => Kind::Slide,
        Tool::Turn => Kind::Turn,
        _ => Kind::None,
    }
}

/// What is drawn on the chosen object this frame, and where the gizmo came out.
///
/// One place, called by the window and by `--ui-shot`: a picture of the marks then has to be a
/// picture of the marks the window draws, which is the only thing that makes such a picture
/// worth taking. `at` is where the object is *drawn* - see `view::Shown::where_drawn`, which
/// is not the same thing as where its map record says it is.
pub fn show_for(
    tool: Tool,
    hot: Option<Handle>,
    chosen: Option<Selection>,
    at: Option<DVec3>,
    bubble: Option<(DVec3, f64)>,
    camera: &Camera,
    height: f32,
) -> (Show, Option<Gizmo>) {
    // nothing chosen, or something taken away - there is nothing to draw on
    let chosen = chosen.filter(|s| !s.edit.deleted);
    let kind = match chosen {
        Some(_) => kind_for(tool),
        None => Kind::None,
    };
    let gizmo = at
        .filter(|_| chosen.is_some() && kind != Kind::None)
        .map(|at| {
            let distance = (camera.position - at).length();
            Gizmo { at, size: Gizmo::size_for(distance, height, camera.fov_deg, SPAN) }
        });
    // the bubble is the mark of a tool that only chooses. A tool with a gizmo on the object
    // has already said which one it is, and a wash drawn under the arrows (the blended pass
    // comes after the opaque one) only dims them
    let bubble = if kind == Kind::None { bubble } else { None };
    let show = Show { bubble, gizmo: gizmo.map(|g| (g.at, g.size)), kind, hot };
    (show, gizmo)
}

/// The editor's marks: a bubble round the chosen object and the gizmo it is dragged by.
///
/// Built once and moved afterwards, deliberately: nothing takes a mesh or a material back out
/// of a `Scene`, so one made per frame would grow the scene for as long as the editor is
/// open. What is not wanted is hidden, which is a flag and not a mesh.
#[derive(Default)]
pub struct Marks {
    built: Option<Built>,
}

struct Built {
    bubble: (MeshId, usize),
    /// One arrow per axis: the same mesh, turned into place, so that the one under the
    /// pointer can be drawn longer on its own.
    arrows: [(MeshId, usize); 3],
    ring: (MeshId, usize),
}

impl Marks {
    /// Draw what `show` asks for.
    pub fn draw(&mut self, renderer: &Renderer, scene: &mut Scene, show: &Show) {
        let built = self.ensure(renderer, scene);
        let scale = |s: f64| Mat4::from_scale(Vec3::splat(s as f32));

        // the bubble round the object: a sphere of its own radius, or nothing
        let bubble = show.bubble.filter(|(_, r)| *r > 0.01);
        // (the pair is (mesh, instance) - the instance is what is moved)
        let (_, inst) = built.bubble;
        if let Some((centre, radius)) = bubble {
            renderer.set_transform(scene, inst, centre, scale(radius));
        }
        renderer.set_params(scene, inst, &[], bubble.is_some(), &[]);

        // the gizmo: one of the two shapes, never both
        let gizmo = show.gizmo().filter(|g| show.kind != Kind::None && g.size > 0.001);
        for (k, axis) in Axis::ALL.iter().enumerate() {
            let (_, inst) = built.arrows[k];
            let on = gizmo.is_some() && show.kind == Kind::Slide;
            if let Some(g) = gizmo.filter(|_| on) {
                let handle = Handle::Slide(*axis);
                renderer.set_transform(scene, inst, g.at, axis.turn() * scale(g.drawn(handle, show.hot)));
            }
            renderer.set_params(scene, inst, &[], on, &[]);
        }
        let (_, ring) = built.ring;
        let ring_on = gizmo.is_some() && show.kind == Kind::Turn;
        if let Some(g) = gizmo.filter(|_| ring_on) {
            renderer.set_transform(scene, ring, g.at, scale(g.drawn(Handle::Turn, show.hot)));
        }
        renderer.set_params(scene, ring, &[], ring_on, &[]);
    }

    /// Forget the marks: they were built against the scene that is being thrown away, and
    /// their mesh and instance numbers mean nothing in the next one.
    pub fn reset(&mut self) {
        self.built = None;
    }

    fn ensure(&mut self, renderer: &Renderer, scene: &mut Scene) -> &Built {
        if self.built.is_none() {
            let bubble_mat = renderer.add_material(scene, None, AlphaMode::Blend, [0.91, 0.63, 0.19, 0.16], true);
            // the bubble is a wash over what is already there rather than a pane in front of
            // it: without this it punches a hole in everything blended drawn after it
            renderer.set_no_z_write(scene, bubble_mat, true);
            let sphere = renderer.add_mesh(scene, &sphere_mesh());
            let arrow = renderer.add_mesh(scene, &arrow_mesh());
            let ring = renderer.add_mesh(scene, &ring_mesh());
            let hidden = Mat4::from_scale(Vec3::ZERO);
            let bubble = (sphere, renderer.add_instance(scene, sphere, DVec3::ZERO, hidden, vec![bubble_mat]));
            let mut arrows = [(arrow, 0usize); 3];
            for (k, axis) in Axis::ALL.iter().enumerate() {
                let mat = renderer.add_material(scene, None, AlphaMode::Opaque, axis.colour(), true);
                let inst = renderer.add_instance(scene, arrow, DVec3::ZERO, hidden, vec![mat]);
                arrows[k] = (arrow, inst);
            }
            let ring_mat = renderer.add_material(scene, None, AlphaMode::Opaque, [0.94, 0.74, 0.28, 1.0], true);
            let ring = (ring, renderer.add_instance(scene, ring, DVec3::ZERO, hidden, vec![ring_mat]));
            // the marks belong to the editor, and nothing of them goes into the map: no sun
            // shadow, and no OMSI caster flags
            for (_, inst) in [bubble, arrows[0], arrows[1], arrows[2], ring] {
                renderer.set_casts_shadow(scene, inst, false);
                renderer.set_params(scene, inst, &[], false, &[]);
            }
            self.built = Some(Built { bubble, arrows, ring });
        }
        self.built.as_ref().expect("just built")
    }
}

// ---- the meshes ---------------------------------------------------------------------------
//
// A unit arrow along +x, a unit ring round z, a unit sphere: a gizmo is placed by scaling one
// of them, so none of them is ever rebuilt.

/// A triangle soup, with the material slots the renderer draws it in.
#[derive(Default)]
struct Soup {
    positions: Vec<Vec3>,
    normals: Vec<Vec3>,
    uvs: Vec<Vec2>,
    indices: Vec<u32>,
    /// (first index, index count, material slot)
    ranges: Vec<(u32, u32, u32)>,
}

impl Soup {
    /// Begin a material slot: every triangle pushed after this is drawn with material `slot`.
    fn slot(&mut self, slot: u32) {
        self.ranges.push((self.indices.len() as u32, 0, slot));
    }

    fn tri(&mut self, a: Vec3, b: Vec3, c: Vec3) {
        if self.ranges.is_empty() {
            self.slot(0);
        }
        let n = (b - a).cross(c - a).normalize_or(Vec3::Z);
        let base = self.positions.len() as u32;
        for p in [a, b, c] {
            self.positions.push(p);
            self.normals.push(n);
            self.uvs.push(Vec2::ZERO);
        }
        self.indices.extend_from_slice(&[base, base + 1, base + 2]);
        let last = self.ranges.last_mut().expect("a slot");
        last.1 += 3;
    }

    fn quad(&mut self, a: Vec3, b: Vec3, c: Vec3, d: Vec3) {
        self.tri(a, b, c);
        self.tri(a, c, d);
    }

    /// A box from `lo` to `hi`: four sides and two ends.
    fn boxed(&mut self, lo: Vec3, hi: Vec3) {
        let c = |x: f32, y: f32, z: f32| Vec3::new(if x > 0.5 { hi.x } else { lo.x }, if y > 0.5 { hi.y } else { lo.y }, if z > 0.5 { hi.z } else { lo.z });
        for (a, b, d, e) in [
            (c(0., 0., 0.), c(1., 0., 0.), c(1., 1., 0.), c(0., 1., 0.)),
            (c(0., 0., 1.), c(0., 1., 1.), c(1., 1., 1.), c(1., 0., 1.)),
            (c(0., 0., 0.), c(0., 0., 1.), c(1., 0., 1.), c(1., 0., 0.)),
            (c(0., 1., 0.), c(1., 1., 0.), c(1., 1., 1.), c(0., 1., 1.)),
            (c(0., 0., 0.), c(0., 1., 0.), c(0., 1., 1.), c(0., 0., 1.)),
            (c(1., 0., 0.), c(1., 0., 1.), c(1., 1., 1.), c(1., 1., 0.)),
        ] {
            self.quad(a, b, d, e);
        }
    }

    fn finish(self) -> MeshData {
        MeshData {
            positions: self.positions,
            normals: self.normals,
            uvs: self.uvs,
            indices: self.indices,
            ranges: self.ranges,
            // the marks are seen from wherever the camera is, so both faces of them are
            one_sided: false,
        }
    }
}

/// A unit sphere: the wash round the chosen object. Eight rings is round enough for a colour
/// without an edge in it.
fn sphere_mesh() -> MeshData {
    let (rings, sectors) = (8usize, 16usize);
    let mut soup = Soup::default();
    soup.slot(0);
    let at = |i: usize, j: usize| {
        let (v, u) = (i as f32 / rings as f32, j as f32 / sectors as f32);
        let (sp, cp) = (std::f32::consts::PI * v).sin_cos();
        let (st, ct) = (std::f32::consts::TAU * u).sin_cos();
        Vec3::new(sp * ct, sp * st, cp)
    };
    for i in 0..rings {
        for j in 0..sectors {
            soup.quad(at(i, j), at(i + 1, j), at(i + 1, j + 1), at(i, j + 1));
        }
    }
    soup.finish()
}

/// One arrow: a shaft along +x with a four-sided head on it. Every axis is drawn with this
/// same mesh, turned into place.
fn arrow_mesh() -> MeshData {
    let mut soup = Soup::default();
    soup.slot(0);
    let (shaft, thick) = (0.76f32, 0.022f32);
    soup.boxed(Vec3::new(0.0, -thick, -thick), Vec3::new(shaft, thick, thick));
    let (base, half) = (shaft, 0.078f32);
    let tip = Vec3::new(1.0, 0.0, 0.0);
    let corners = [
        Vec3::new(base, -half, -half),
        Vec3::new(base, half, -half),
        Vec3::new(base, half, half),
        Vec3::new(base, -half, half),
    ];
    for k in 0..4 {
        soup.tri(corners[k], corners[(k + 1) % 4], tip);
    }
    soup.quad(corners[3], corners[2], corners[1], corners[0]);
    soup.finish()
}

/// A unit ring round z: a short tube, so it is a ring seen from anywhere. Flat on the ground
/// it would vanish edge-on, which is just the view a modder turns things from.
fn ring_mesh() -> MeshData {
    let (major, minor) = (40usize, 5usize);
    let tube = 0.024f32;
    let mut soup = Soup::default();
    soup.slot(0);
    let at = |i: usize, j: usize| {
        let u = std::f32::consts::TAU * i as f32 / major as f32;
        let v = std::f32::consts::TAU * j as f32 / minor as f32;
        let (su, cu) = u.sin_cos();
        let (sv, cv) = v.sin_cos();
        let r = 1.0 + tube * cv;
        Vec3::new(r * su, r * cu, tube * sv)
    };
    for i in 0..major {
        for j in 0..minor {
            soup.quad(at(i, j), at(i, j + 1), at(i + 1, j + 1), at(i + 1, j));
        }
    }
    soup.finish()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A camera level and looking north, 20 m up and 40 m south of the origin.
    fn camera() -> Camera {
        Camera { position: DVec3::new(0.0, -40.0, 20.0), yaw: 0.0, pitch: -26.565, roll: 0.0, fov_deg: 60.0, near: 0.5, far: 8000.0 }
    }

    fn screen() -> Screen {
        Screen::new(&camera(), camera().position, 1600, 900)
    }

    #[test]
    fn what_is_straight_ahead_is_the_middle_of_the_screen() {
        let c = camera();
        let s = Screen::new(&c, c.position, 1600, 900);
        let ahead = c.position + c.forward().as_dvec3() * 100.0;
        let p = s.project(ahead).expect("in front of the camera");
        assert!((p.x - 800.0).abs() < 0.5 && (p.y - 450.0).abs() < 0.5, "expected the middle, got {p:?}");
    }

    #[test]
    fn what_is_behind_the_camera_has_no_place_on_the_screen() {
        let c = camera();
        let s = Screen::new(&c, c.position, 1600, 900);
        assert!(s.project(c.position - c.forward().as_dvec3() * 50.0).is_none());
    }

    #[test]
    fn an_arrow_is_got_hold_of_where_it_is_drawn() {
        let (s, g) = (screen(), Gizmo { at: DVec3::ZERO, size: 10.0 });
        let on = s.project(g.at + DVec3::X * 5.0).expect("on the screen");
        assert_eq!(g.pick(&s, Kind::Slide, on, None), Some(Handle::Slide(Axis::East)), "halfway along the east arrow");
        // a place well off every arrow - six metres west, which is behind all three
        let off = s.project(g.at - DVec3::X * 6.0).expect("on the screen");
        assert_eq!(g.pick(&s, Kind::Slide, off, None), None, "nothing is drawn there");
    }

    #[test]
    fn the_ring_is_got_hold_of_where_it_is_drawn() {
        let (s, g) = (screen(), Gizmo { at: DVec3::ZERO, size: 10.0 });
        // a place on the ring: the east point of it, and a quarter of the way round
        let on = s.project(g.at + DVec3::X * 10.0).expect("on the screen");
        assert_eq!(g.pick(&s, Kind::Turn, on, None), Some(Handle::Turn));
        // and the middle of the disc is nowhere near the ring
        let inside = s.project(g.at).expect("on the screen");
        assert_eq!(g.pick(&s, Kind::Turn, inside, None), None, "the middle of the ring is not the ring");
    }

    #[test]
    fn a_ring_that_is_not_drawn_is_not_picked_up_under_an_arrows_tip() {
        // the arrows and the ring come out the same length, so the east arrow's tip is also
        // the east point of the ring - and only one of the two is ever on the screen
        let (s, g) = (screen(), Gizmo { at: DVec3::ZERO, size: 10.0 });
        let tip = s.project(g.at + DVec3::X * 10.0).expect("on the screen");
        assert_eq!(g.pick(&s, Kind::Slide, tip, None), Some(Handle::Slide(Axis::East)));
        assert_eq!(g.pick(&s, Kind::Turn, tip, None), Some(Handle::Turn));
        assert_eq!(g.pick(&s, Kind::None, tip, None), None, "nothing is drawn at all");
    }

    #[test]
    fn the_arrow_under_the_pointer_stays_under_it_when_it_grows() {
        let (s, g) = (screen(), Gizmo { at: DVec3::ZERO, size: 10.0 });
        let hot = Handle::Slide(Axis::East);
        // the place the tip has grown to is the place the pick looks at for a drawn-longer
        // handle - without it the tip would grow away from the pointer and flicker
        let grown = s.project(g.point(hot, 1.0, Some(hot))).expect("on the screen");
        assert_eq!(g.pick(&s, Kind::Slide, grown, None), None, "not where the plain arrow ends");
        assert_eq!(g.pick(&s, Kind::Slide, grown, Some(hot)), Some(hot), "but where the drawn one does");
    }

    #[test]
    fn dragging_along_an_arrow_gives_the_distance_it_was_dragged() {
        let (s, g) = (screen(), Gizmo { at: DVec3::ZERO, size: 10.0 });
        let grab = s.project(g.at).expect("on the screen");
        let to = s.project(g.at + DVec3::X * 10.0).expect("on the screen");
        let got = g.slide(&s, Axis::East, grab, to).expect("the east arrow is on the screen");
        assert!((got - 10.0).abs() < 0.01, "expected 10 m, got {got}");
        let back = g.slide(&s, Axis::East, to, grab).expect("still on the screen");
        assert!((back + 10.0).abs() < 0.01, "expected -10 m, got {back}");
    }

    #[test]
    fn an_axis_pointing_at_the_camera_cannot_be_dragged() {
        let looking = Camera { position: DVec3::new(-100.0, 0.0, 0.0), yaw: 90.0, pitch: 0.0, roll: 0.0, fov_deg: 60.0, near: 0.5, far: 8000.0 };
        let s = Screen::new(&looking, looking.position, 1600, 900);
        let g = Gizmo { at: DVec3::ZERO, size: 10.0 };
        assert!(g.slide(&s, Axis::East, Vec2::new(800.0, 450.0), Vec2::new(900.0, 450.0)).is_none());
    }

    #[test]
    fn a_turn_is_the_way_the_pointer_went() {
        let at = DVec3::ZERO;
        // heading 0 is north (+y); a pointer that came round to the east asks for +90
        assert!((turn(at, DVec3::Y * 5.0, DVec3::X * 5.0) - 90.0).abs() < 0.001);
        assert!((turn(at, DVec3::X * 5.0, DVec3::Y * 5.0) + 90.0).abs() < 0.001);
        // and past the near side it is the short way round
        assert!((turn(at, DVec3::Y * 5.0, DVec3::Y * 5.0 + DVec3::X * 0.1) - 1.1458).abs() < 0.01);
    }

    #[test]
    fn the_gizmo_comes_out_the_same_size_on_the_screen_from_any_distance() {
        let one = Gizmo::size_for(100.0, 900.0, 60.0, SPAN);
        let far = Gizmo::size_for(200.0, 900.0, 60.0, SPAN);
        assert!((far - one * 2.0).abs() < 1e-9, "twice as far is twice as long, so it looks the same");
        // a metre at 100 m through this lens: 900 / (2 tan 30) pixels
        let per_metre = 900.0 / (2.0 * (30f64.to_radians()).tan());
        assert!(one > 100.0 * SPAN as f64 / per_metre * 0.9);
        assert!(one < 100.0 * SPAN as f64 / per_metre * 1.1);
    }
}
