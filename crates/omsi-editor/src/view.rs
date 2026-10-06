//! The map as the editor draws it: the tiles kept loaded around the camera, and the camera.
//!
//! The tiles themselves come from the game - `openomsi_game::host` opens the same [`World`] the
//! game opens and streams it with the same `build_scene` / `unload_tile`. This module only
//! decides *which* tiles that is, and where the camera looks. That split is deliberate: if
//! the editor drew a tile its own way, a modder would place something that looked right here
//! and wrong in the game.
//!
//! Nothing here knows about editing. A [`View`] can be opened on a map and flown around
//! without a single edit being made.

use anyhow::Result;
use glam::{DVec3, Mat4};
use hashbrown::HashSet;
use openomsi_game::host::{self, LoadStats, TileGpu, World};
use omsi_editor_core::{ObjectEdit, Session, TileId};
use omsi_render::{Camera, Lighting, Renderer, Scene};
use std::path::Path;

/// How far around the camera tiles are kept loaded, in tiles (Chebyshev). A tile is 300 m in
/// a stock map, so this is the map's "visible distance" and then some.
pub const RADIUS: i32 = 3;

/// How far the editor's air lets you see, in metres.
///
/// The game takes this from the weather, which at a stock map's haze puts a tile under a
/// kilometre away into a white wash - fine when driving through it, useless when placing an
/// object against it. An editor is inspecting geometry, so it looks through clear air; the
/// sun, the sky and the colours still come from the map's own date and place.
pub const FOG_RANGE: f32 = 8000.0;

/// The camera's speed at the default zoom, m/s, and how far the wheel can take it.
const SPEED: f64 = 15.0;
const SPEED_MIN: f64 = 1.0;
const SPEED_MAX: f64 = 400.0;

/// How far the pointer turns the view, in degrees per pixel.
const LOOK_GAIN: f64 = 0.15;

// ---- flying ---------------------------------------------------------------------------
//
// Both of these are plain functions of a camera, with no map and no window about them, so
// the direction they go in can be checked on its own - which is the thing worth checking:
// a stray minus sign here reads as the map sliding the wrong way under the pointer, and
// nothing else in the program would show it.

/// Turn a camera by a movement of the pointer, in pixels: the view comes round to the right
/// as the pointer goes right, and up as it goes up.
///
/// This is the game's own free camera's rule, at its own gain, so the two turn alike.
pub fn turn(camera: &mut Camera, dx: f64, dy: f64) {
    camera.yaw = (camera.yaw + (dx * LOOK_GAIN) as f32).rem_euclid(360.0);
    camera.pitch = (camera.pitch - (dy * LOOK_GAIN) as f32).clamp(-89.0, 89.0);
}

/// Move a camera along its own axes: `forward` along the view, `right` towards its right
/// hand, `up` world-wards (each -1..1), `speed` in metres a second.
///
/// "Right" is the camera's own ([`Camera::right`]) rather than a cross product worked out
/// here: it is the same right that orders the picture, so D cannot end up moving left.
pub fn advance(camera: &mut Camera, speed: f64, forward: f64, right: f64, up: f64, dt: f64) {
    if forward == 0.0 && right == 0.0 && up == 0.0 {
        return;
    }
    let step = speed * dt;
    let (f, r) = (camera.forward().as_dvec3(), camera.right().as_dvec3());
    camera.position += (f * forward + r * right) * step + DVec3::Z * (up * step);
}

/// The middle of the map's tiles, for a map with no entry point to start from.
fn centre_of(world: &World) -> DVec3 {
    let size = omsi_map::tile_size();
    let tiles = &world.global.tiles;
    if tiles.is_empty() {
        return DVec3::ZERO;
    }
    let (sx, sy) = tiles.iter().fold((0.0, 0.0), |(x, y), t| (x + t.x as f64, y + t.y as f64));
    let n = tiles.len() as f64;
    DVec3::new((sx / n + 0.5) * size, (sy / n + 0.5) * size, 0.0)
}

/// The map drawn, and where it is being looked at from.
pub struct View {
    world: World,
    camera: Camera,
    /// The tiles currently on the GPU, and the area that was asked for last time - the second
    /// is what makes [`View::stream`] cheap to call every frame: nothing is asked for until
    /// the camera has moved into another tile.
    loaded: HashSet<(i32, i32)>,
    asked_for: Option<(i32, i32)>,
    /// The tiles that were *there* - listed and read from a file - the last time the world's
    /// layout was believed. A tile that has appeared in that list since, or one that has
    /// become readable, leaves the layout behind (`View::adopt_tiles`).
    tiles_there: Option<Vec<(i32, i32)>>,
    day_of_year: i32,
    pub speed: f64,
    /// The last thing that happened, for the window's title.
    pub note: String,
}

/// What the window is already showing of the session's changes.
///
/// The in-game editor keeps the same list, for the same reason: a map holds tens of thousands
/// of objects, and a frame in which nothing was changed must not touch any of them. Only what
/// the session has actually done lives here, and only it is looked at.
#[derive(Default)]
pub struct Shown {
    /// The edit each object was last drawn with.
    objects: hashbrown::HashMap<i64, ObjectEdit>,
    /// The helper object each copy placed in this session was drawn as, and where it was put -
    /// a copy is moved and taken away like anything else, and its edit lives on the copy
    /// itself rather than in the map's edit table (see `Document::edit`).
    placed: hashbrown::HashMap<i64, Placed>,
    /// A signature of the ground the last time each tile's was drawn.
    ground: hashbrown::HashMap<(i32, i32), u64>,
}

/// A copy placed this session as it is drawn: the helper object, and the place and heading it
/// was put at.
struct Placed {
    gpu: TileGpu,
    at: DVec3,
    heading: f64,
}

impl Shown {
    /// Where object `id` is drawn, in the world - what a gizmo is put on. `None` for something
    /// that is not on the screen at all: taken away, or in a tile that is not loaded.
    ///
    /// Not `Selection::position()`. A map record's coordinates are *tile-local* - they are what
    /// the tile file writes, and OMSI reads them against the tile's own place on the map - so a
    /// gizmo set on them stands up to a few tiles away from the object, off in the next field.
    /// The world's own object index has the place the tile put it, ground height and all, which
    /// is exactly where it is drawn. A copy placed this session is answered for out of its own
    /// helper object, because it is not in that index until it is saved.
    pub fn where_drawn(&self, world: &World, id: i64) -> Option<DVec3> {
        if let Some(p) = self.placed.get(&id) {
            return Some(p.at);
        }
        // (the same order `World::apply_object_edit` takes them in, so the two cannot meet
        // head-on and wait for each other)
        let moved = world.object_edits.lock().get(&id).copied().unwrap_or_default().moved;
        let objects = world.edit_objects.lock();
        Some(objects.get(&id)?.pos + moved)
    }

    /// The sphere round everything object `id` is drawn with - what a front end draws its
    /// selection mark on.
    pub fn bubble(&self, world: &World, scene: &Scene, id: i64) -> Option<(DVec3, f64)> {
        if let Some(p) = self.placed.get(&id) {
            return crate::gizmo::bubble(scene, &p.gpu.instances);
        }
        let objects = world.edit_objects.lock();
        let eo = objects.get(&id)?;
        crate::gizmo::bubble(scene, &eo.instances)
    }

    /// The instances object `id` is drawn with - what the outline is drawn round, one ring
    /// being several instances wide (see `Scene::outline`).
    ///
    /// A copy placed this session answers out of its own helper object, as `bubble` does: it is
    /// not in the world's index until the map is saved.
    pub fn instances_of(&self, world: &World, id: i64) -> Vec<usize> {
        if let Some(p) = self.placed.get(&id) {
            return p.gpu.instances.clone();
        }
        world
            .edit_objects
            .lock()
            .get(&id)
            .map(|o| o.instances.clone())
            .unwrap_or_default()
    }
}

impl View {
    /// Put what the session has changed on the screen: an object it moved or took away, a copy
    /// it placed, the ground it shaped - and, just as much, what it has put *back*, so that
    /// taking a change back leaves the picture as it leaves the map.
    ///
    /// Drawing is the game's own - `apply_object_edit` is the very call the in-game editor
    /// makes while dragging an object - so what the modder sees here is what the game will
    /// show. Nothing is done for a change that is already on the screen.
    pub fn sync(&mut self, session: &Session, shown: &mut Shown, renderer: &Renderer, scene: &mut Scene) {
        // An edit that has been taken back to nothing leaves the map's list of edits
        // altogether (see `Document::set_edit`), so that list is not enough on its own: the
        // object whose move was just undone is simply *absent* from it, and walking it alone
        // would leave the object standing where the undone change put it - the picture would
        // say the undo had not happened at all. The ids the view is still holding are
        // therefore walked as well, and one the map no longer lists is put back where its
        // record has said all along that it stands.
        let edits: Vec<(i64, ObjectEdit)> = session.doc().edits().collect();
        let listed: HashSet<i64> = edits.iter().map(|&(id, _)| id).collect();
        for &(id, edit) in &edits {
            if shown.objects.get(&id) == Some(&edit) {
                continue;
            }
            shown.objects.insert(id, edit);
            self.world.apply_object_edit(renderer, scene, id, edit);
        }
        let put_back: Vec<i64> = shown.objects.keys().copied().filter(|id| !listed.contains(id)).collect();
        for id in put_back {
            shown.objects.remove(&id);
            self.world.apply_object_edit(renderer, scene, id, ObjectEdit::default());
        }
        // the copies placed this session: put down once, then moved and taken back as the
        // session says. A copy's edit is on the copy itself, so it never comes round through
        // `edits()` above - which is why it is followed here.
        let mut standing: Vec<i64> = Vec::new();
        for a in session.doc().added() {
            standing.push(a.id);
            let (at, heading) = (a.position(), a.heading());
            match shown.placed.get_mut(&a.id) {
                Some(p) if p.at == at && p.heading == heading => {}
                Some(p) => {
                    // moved or turned: its instances go with it, exactly as `show_edit` moves a
                    // map object's
                    let rot = Mat4::from_rotation_z(-(heading.to_radians() as f32));
                    for inst in &p.gpu.instances {
                        renderer.set_transform(scene, *inst, at, rot);
                    }
                    p.at = at;
                    p.heading = heading;
                }
                None => {
                    if let Some(g) = self.world.add_helper_object(renderer, scene, &a.sco, at, heading, &[]) {
                        shown.placed.insert(a.id, Placed { gpu: g, at, heading });
                    }
                }
            }
        }
        let gone: Vec<i64> = shown.placed.keys().copied().filter(|id| !standing.contains(id)).collect();
        for id in gone {
            if let Some(p) = shown.placed.remove(&id) {
                self.world.remove_helper_object(renderer, scene, p.gpu);
            }
        }
        // ground is part of a tile's mesh, so a shaped tile is read again rather than nudged -
        // and a tile whose ground has been put back exactly is not "changed" any more at all
        // (a change is measured against the hash the map was read with, see
        // `Document::ground_is_dirty`), so it would drop out of the list below and go on being
        // drawn shaped. It is read again for the same reason an object is put back from its
        // record above: what is on the screen is what the map now says, not what was last done
        // to it.
        let ground: Vec<(TileId, &omsi_map::Terrain)> = session.doc().changed_ground().collect();
        let shaped: HashSet<TileId> = ground.iter().map(|&(id, _)| id).collect();
        for &(id, t) in &ground {
            let h = ground_hash(t);
            if shown.ground.get(&id) == Some(&h) {
                continue;
            }
            shown.ground.insert(id, h);
            self.world.terrain_edits.lock().insert(id, t.clone());
            self.reground(renderer, scene, id);
        }
        let unshaped: Vec<TileId> = shown.ground.keys().copied().filter(|id| !shaped.contains(id)).collect();
        for id in unshaped {
            shown.ground.remove(&id);
            self.world.terrain_edits.lock().remove(&id);
            self.reground(renderer, scene, id);
        }
    }

    /// Read one tile again so that a change to its ground shows. A tile that is not on the
    /// screen costs nothing: it will be read with the new ground when it is next wanted.
    fn reground(&mut self, renderer: &Renderer, scene: &mut Scene, id: (i32, i32)) {
        if !self.loaded.contains(&id) {
            return;
        }
        let Some(path) = self.world.tile_source(id.0, id.1) else { return };
        host::unload(&self.world, renderer, scene, id);
        match host::build(&self.world, renderer, scene, &[(id.0, id.1, path)]) {
            Ok(s) => self.note = format!("ground of tile ({}, {}): {} objects", id.0, id.1, s.objects),
            Err(e) => {
                log::error!("reading tile ({}, {}) again: {e:#}", id.0, id.1);
                self.note = format!("{e:#}");
            }
        }
    }
}

/// A signature of a height field, for "is this ground still what was drawn".
fn ground_hash(t: &omsi_map::Terrain) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for v in &t.heights {
        h ^= v.to_bits() as u64;
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    h
}

impl View {
    /// Open the map and draw the tiles around its first entry point (or around the origin
    /// when it has none).
    pub fn open(
        root: &Path,
        map_cfg: &Path,
        renderer: &Renderer,
        scene: &mut Scene,
    ) -> Result<Self> {
        let clock = host::clock();
        let world = host::open(root, map_cfg)?;
        let start = world
            .global
            .entry_points
            .first()
            .and_then(|ep| world.entry_point_place(ep))
            .map(|(p, _)| p)
            .unwrap_or_else(|| centre_of(&world));
        let mut view = View {
            world,
            camera: host::camera_over(start, 60.0),
            loaded: HashSet::new(),
            asked_for: None,
            tiles_there: None,
            day_of_year: clock.day_of_year,
            speed: SPEED,
            note: String::new(),
        };
        // the first load is the slow one and worth reporting
        let stats = view.stream(renderer, scene);
        log::info!(
            "{}: {} tiles, {} objects, {} splines, {} textures{}",
            view.world.global.name,
            stats.tiles,
            stats.objects,
            stats.splines,
            stats.textures,
            if stats.failed_objects > 0 {
                format!(", {} object(s) unresolved", stats.failed_objects)
            } else {
                String::new()
            }
        );
        Ok(view)
    }

    pub fn world(&self) -> &World {
        &self.world
    }

    pub fn camera(&self) -> &Camera {
        &self.camera
    }

    /// How many tiles are on the screen.
    pub fn loaded_count(&self) -> usize {
        self.loaded.len()
    }

    /// The light of the hour the view was opened at. The map's own time zone and place decide
    /// where the sun is, so this is computed rather than picked.
    pub fn lighting(&self) -> Lighting {
        self.lighting_at(host::clock().hour() as f64)
    }

    /// The light of a given hour of the day, on the day the view was opened on.
    pub fn lighting_at(&self, hour: f64) -> Lighting {
        host::lighting(self.day_of_year, hour, FOG_RANGE)
    }

    /// Bring the tiles in line with where the camera is now.
    ///
    /// Cheap when the camera has not left the tile it was in: the whole point of keeping the
    /// requested area rather than the tile list is that a flying camera calls this every
    /// frame and it usually does nothing at all.
    pub fn stream(&mut self, renderer: &Renderer, scene: &mut Scene) -> LoadStats {
        let here = host::tile_of(self.camera.position);
        if self.asked_for == Some(here) {
            return LoadStats::default();
        }
        self.asked_for = Some(here);
        let want = host::tiles_near(&self.world, self.camera.position, RADIUS);
        let wanted: HashSet<(i32, i32)> = want.iter().map(|t| (t.0, t.1)).collect();

        // what the camera has left behind goes first, so a long flight does not hold two
        // areas' worth of textures
        let gone: Vec<(i32, i32)> = self.loaded.difference(&wanted).copied().collect();
        for key in &gone {
            host::unload(&self.world, renderer, scene, *key);
        }
        if !gone.is_empty() {
            host::release_types(&self.world);
        }
        self.loaded = self.loaded.intersection(&wanted).copied().collect();

        let fresh: Vec<(i32, i32, std::path::PathBuf)> = want
            .into_iter()
            .filter(|t| !self.loaded.contains(&(t.0, t.1)))
            .collect();
        let stats = if fresh.is_empty() {
            LoadStats::default()
        } else {
            let keys: Vec<(i32, i32)> = fresh.iter().map(|t| (t.0, t.1)).collect();
            let stats = match host::build(&self.world, renderer, scene, &fresh) {
                Ok(s) => s,
                Err(e) => {
                    // one bad tile must not take the whole map off the screen
                    log::error!("tiles {keys:?}: {e:#}");
                    self.note = format!("{e:#}");
                    LoadStats::default()
                }
            };
            self.loaded.extend(keys);
            stats
        };
        self.note = format!(
            "{} tiles loaded, {} objects",
            self.loaded.len(),
            stats.objects
        );
        stats
    }

    /// Every tile of the map at once - "show me the whole thing", which a radius cannot do.
    pub fn stream_all(&mut self, renderer: &Renderer, scene: &mut Scene) {
        let all = host::tiles_all(&self.world);
        let keys: Vec<(i32, i32)> = all.iter().map(|t| (t.0, t.1)).collect();
        let fresh: Vec<_> = all
            .into_iter()
            .filter(|t| !self.loaded.contains(&(t.0, t.1)))
            .collect();
        if fresh.is_empty() {
            return;
        }
        match host::build(&self.world, renderer, scene, &fresh) {
            Ok(s) => {
                self.loaded.extend(keys);
                self.note = format!("the whole map: {} tiles, {} objects", self.loaded.len(), s.objects);
            }
            Err(e) => {
                log::error!("{e:#}");
                self.note = format!("{e:#}");
            }
        }
        self.asked_for = None;
    }

    /// Take the map's own list of tiles from the session: the tiles a world streams, the
    /// numbering an entry point counts to, and the entry points.
    ///
    /// A [`World`] holds that list as it was when the map was opened, and streams what it
    /// names - so a tile added to the list was not drawn, and one taken off it went on being
    /// drawn, until the map was opened again. This is what puts the two back in step.
    ///
    /// Nothing is built or unloaded here. The streaming that is already there does that, and
    /// does it right, because a tile that went is outside what is wanted and one that came is
    /// inside it (see [`View::stream`]). The one thing that has to be said is that the area
    /// worth having is worth working out again: `stream` does nothing while the camera stands
    /// in the tile it was last in, and the camera has not moved.
    ///
    /// A tile added this session has no file until a save writes one, and a tile that is not
    /// on disk cannot be built: a tile added and not yet written is in the list and not on the
    /// ground, which is the state the tile grid draws in the meantime.
    pub fn adopt_tiles(&mut self, session: &Session) {
        let now = session.doc().global_now();
        let relisted = now.tiles != self.world.global.tiles;
        // the three fields that name tiles: what each is called, what number a tile index
        // counts to, and which tile each entry point stands in
        self.world.global.tiles = now.tiles;
        self.world.global.raw_tiles = now.raw_tiles;
        self.world.global.entry_points = now.entry_points;
        // The world also holds a layout built from the tiles that were *there* when it was
        // first asked for one - each tile's file, its neighbours, what its ground is cut with -
        // and a load goes through that layout and no other: a tile with no place in it is
        // staged as nothing and drawn as nothing. A tile added to the list has no place in it,
        // and neither has a tile whose file a save has just written, because until then it was
        // a tile that could not be built at all. So the layout goes when either of those has
        // happened - which is asked of the files themselves rather than remembered, since what
        // makes a tile a tile is that its file is there.
        let there: Vec<TileId> = self.world.select_tiles(None, None).into_iter().map(|(x, y, _)| (x, y)).collect();
        // (the first look is not a change: the layout was built by the load that opened the
        // map, from this very list)
        let stale = relisted || self.tiles_there.as_ref().is_some_and(|then| *then != there);
        self.tiles_there = Some(there);
        if stale {
            self.world.forget_layout();
        }
        // the tile the camera asked for last is about to be the wrong answer, and asking it is
        // the whole of what `stream` checks before deciding there is nothing to do
        self.asked_for = None;
    }

    // ---- the camera -------------------------------------------------------------------

    /// Fly: `forward` along the view, `right` across it to the right hand, `up` world-wards
    /// (each -1..1).
    pub fn fly(&mut self, forward: f64, right: f64, up: f64, dt: f64) {
        advance(&mut self.camera, self.speed, forward, right, up, dt);
    }

    /// Turn the view by a movement of the pointer, in pixels.
    pub fn look(&mut self, dx: f64, dy: f64) {
        turn(&mut self.camera, dx, dy);
    }

    /// The wheel: the camera's speed, not its field of view - an editor is flown, and a
    /// change of pace is felt where a zoom is only noticed.
    pub fn change_speed(&mut self, ticks: f64) {
        self.speed = (self.speed * 1.25f64.powf(ticks)).clamp(SPEED_MIN, SPEED_MAX);
    }

    /// Point the view at a place from `height` metres up.
    pub fn look_at(&mut self, at: DVec3, height: f64) {
        self.camera.position = at + DVec3::Z * height;
        self.asked_for = None;
    }

    /// Stand `height` metres above `at` and look straight down at it.
    ///
    /// The crosshair then rests on that very place, which is what "look at this spot" means
    /// when the point is to have something under it - picking, and the mark drawn on what was
    /// picked. `look_at` names a place for a *picture* of the map and keeps the camera's own
    /// tilt, so its crosshair lands eighty metres past the place it names; this is the other
    /// question, and `--ui-aim` asks it.
    pub fn hover_over(&mut self, at: DVec3, height: f64) {
        self.camera.position = at + DVec3::Z * height;
        self.camera.yaw = 0.0;
        self.camera.pitch = -89.0;
        self.asked_for = None;
    }

    /// Where the middle of the view meets the ground, within 4 km. Kept for what only wants to
    /// know where the view is pointed without saying from where - everything that answers to
    /// the pointer goes through `aim_along`.
    ///
    /// The ray walk is the ground brush's own, from `omsi-editor-core`: the editor aims the
    /// brush and picks an object with exactly the point the game's in-game editor would.
    pub fn aim(&self) -> Option<DVec3> {
        self.aim_along((0.0, 0.0), 1.0)
    }

    /// Where the ray through a place in the frame meets the ground, within 4 km - what a click
    /// means. `ndc` is -1..1 across the picture, as the pointer arrives in (see `ray`); the
    /// middle of the view is `(0, 0)`.
    ///
    /// The pointer's own aim rather than the middle of the view's: an editor aims with the
    /// pointer, and the outline that says what is under it (see `Scene::outline`) has to name
    /// the same place a click would.
    pub fn aim_along(&self, ndc: (f32, f32), aspect: f32) -> Option<DVec3> {
        let (eye, dir) = self.ray(ndc, aspect);
        omsi_editor_core::ground::aim(
            |x, y| self.world.ground_terrain(x, y),
            eye,
            dir,
            4000.0,
        )
    }

    /// The ray through a place in the frame: `ndc` is -1..1 across the picture, as a click
    /// arrives in. It comes from the camera the picture is drawn with, so a drag that names a
    /// place on the ground names the place the pointer is on.
    pub fn ray(&self, ndc: (f32, f32), aspect: f32) -> (DVec3, DVec3) {
        // the ray is cast from the camera's own position: the projection is only used for its
        // direction, which is what makes this independent of where the frame's origin is
        let (_, direction) = self.camera.ray(ndc.0, ndc.1, aspect, self.camera.position);
        (self.camera.position, direction.as_dvec3())
    }

    /// Where the ray through `ndc` meets the horizontal plane at height `z` - the plane a turn
    /// is measured on. `None` when the view is edge-on to that plane, where there is no place
    /// to have aimed at.
    pub fn plane(&self, ndc: (f32, f32), aspect: f32, z: f64) -> Option<DVec3> {
        let (eye, dir) = self.ray(ndc, aspect);
        if dir.z.abs() < 1e-4 {
            return None;
        }
        let t = (z - eye.z) / dir.z;
        // behind the camera is not a place anyone pointed at either
        (t > 0.01).then(|| eye + dir * t)
    }

    /// Where the ray through `ndc` meets the level the map's tiles are drawn on: the height of
    /// the ground under the camera, or sea level when it is standing over none.
    ///
    /// [`View::aim`] and this are two different questions, and a map's edge is where they part.
    /// `aim` answers "where would a click land", which is ground - and outside the tiles a map
    /// has there is none, so over the ring of tiles it *could* have, `aim` answers nothing at
    /// all. That ring is exactly where the tile tool is used (see `crate::tiles`), so that tool
    /// asks for a level instead, and a level is there whether the map is or not. The camera's
    /// own ground height is what makes the level the one the near tiles are drawn on, rather
    /// than sea level under a map that sits on a hill.
    pub fn aim_on_the_tiles(&self, ndc: (f32, f32), aspect: f32) -> Option<DVec3> {
        let z = self
            .world
            .ground_terrain(self.camera.position.x, self.camera.position.y)
            .unwrap_or(0.0);
        self.plane(ndc, aspect, z)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A level camera at the origin. yaw 0 faces north (+y) and yaw grows clockwise, so 90
    /// faces east (+x) - the map's own headings.
    fn level(yaw: f32) -> Camera {
        Camera {
            position: DVec3::ZERO,
            yaw,
            pitch: 0.0,
            roll: 0.0,
            fov_deg: 60.0,
            near: 0.1,
            far: 10_000.0,
        }
    }

    #[test]
    fn facing_north_the_right_hand_is_east() {
        let right = level(0.0).right();
        assert!(right.x > 0.99 && right.y.abs() < 0.01, "right of north is east, got {right:?}");
    }

    #[test]
    fn d_goes_right_and_a_goes_left() {
        let mut east = level(0.0);
        advance(&mut east, 10.0, 0.0, 1.0, 0.0, 1.0);
        assert!(east.position.x > 9.9, "D facing north should go east, ended at {:?}", east.position);
        let mut west = level(0.0);
        advance(&mut west, 10.0, 0.0, -1.0, 0.0, 1.0);
        assert!(west.position.x < -9.9, "A facing north should go west, ended at {:?}", west.position);
    }

    #[test]
    fn w_goes_forward_and_e_goes_up() {
        let mut north = level(0.0);
        advance(&mut north, 10.0, 1.0, 0.0, 0.0, 1.0);
        assert!(north.position.y > 9.9, "W facing north should go north, ended at {:?}", north.position);
        let mut up = level(0.0);
        advance(&mut up, 10.0, 0.0, 0.0, 1.0, 1.0);
        assert!(up.position.z > 9.9, "E should climb, ended at {:?}", up.position);
    }

    #[test]
    fn the_pointer_turns_the_view_the_way_it_moves() {
        let mut rightwards = level(0.0);
        turn(&mut rightwards, 10.0, 0.0);
        assert!(rightwards.yaw > 0.0, "the pointer going right turns the view clockwise, got yaw {}", rightwards.yaw);
        assert!(rightwards.forward().x > 0.0, "and it comes round towards east, {:?}", rightwards.forward());
        let mut downwards = level(0.0);
        turn(&mut downwards, 0.0, 10.0);
        assert!(downwards.pitch < 0.0, "the pointer going down looks down, got pitch {}", downwards.pitch);
    }

    // ---- the map's own list of tiles, as the world is told it -----------------------------
    //
    // A world streams the tile list of the file the map was opened on, so a tile this session
    // added was invisible and one it took out went on being drawn until the map was opened
    // again. What is checked here is the part that cannot be seen in a picture: the list the
    // world ends up holding, and the numbering an entry point counts with.

    /// A map of two tiles with an entry point standing in the second, so that taking the first
    /// out renumbers it - the thing a world's list has to follow, and the reason a removal is
    /// not just a shorter list. (An entry point record is its index, object, unknown, x, z, y,
    /// four quaternions, the tile it stands in, and its name.)
    const TWO_TILES: &str = "[entrypoints]\r\n1\r\n0\r\n0\r\n0\r\n150\r\n0\r\n150\r\n0\r\n0\r\n0\r\n1\r\n1\r\nStart\r\n\r\n[map]\r\n0\r\n0\r\ntile_0_0.map\r\n\r\n[map]\r\n1\r\n0\r\ntile_1_0.map\r\n";

    /// That map on disk. Nothing here draws a tile, so a tile's own file need only exist: the
    /// world offers the tiles whose files are there (`World::select_tiles`).
    fn a_map_on_disk(name: &str) -> std::path::PathBuf {
        let root = std::env::temp_dir().join(format!("openomsi-view-test-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let map = root.join("maps").join("Tiny");
        std::fs::create_dir_all(&map).unwrap();
        std::fs::write(map.join("global.cfg"), TWO_TILES).unwrap();
        for t in ["tile_0_0.map", "tile_1_0.map"] {
            std::fs::write(map.join(t), "[version]\r\n11\r\n").unwrap();
        }
        root
    }

    /// A view over a world, with no renderer and no frame: what is under test is the list a
    /// world holds, and nothing is drawn in these tests.
    fn a_view_over(world: World) -> View {
        View {
            world,
            camera: host::camera_over(DVec3::ZERO, 60.0),
            loaded: HashSet::new(),
            asked_for: None,
            tiles_there: None,
            day_of_year: 1,
            speed: SPEED,
            note: String::new(),
        }
    }

    /// The tiles a world would stream, in the order it lists them.
    fn listed(world: &World) -> Vec<(i32, i32)> {
        world.global.tiles.iter().map(|t| (t.x, t.y)).collect()
    }

    #[test]
    fn the_worlds_list_of_tiles_follows_the_map_rather_than_the_file_it_was_opened_on() {
        let root = a_map_on_disk("adopt");
        let map_cfg = root.join("maps").join("Tiny").join("global.cfg");
        let mut session = Session::open(&map_cfg, omsi_editor_core::Destination::Content(root.join("content"))).unwrap();
        let mut view = a_view_over(host::open(&root, &map_cfg).unwrap());

        // the map it was opened on: two tiles, and an entry point named for the second
        assert_eq!(listed(view.world()), vec![(0, 0), (1, 0)]);
        assert_eq!(view.world().global.entry_points[0].group, 1);

        // a tile added goes on the end, and there is nothing after it to renumber
        session.add_tile((2, 0)).unwrap();
        view.adopt_tiles(&session);
        assert_eq!(listed(view.world()), vec![(0, 0), (1, 0), (2, 0)]);
        assert_eq!(view.world().global.entry_points[0].group, 1);

        // the first one out: the tiles after it move up the list, so the entry point that
        // counted to 1 counts to 0 - the same tile it named before, by its new number. A world
        // that was only told the shorter list would have it standing in tile (0, 0), which is
        // not in the map at all
        session.remove_tile((0, 0)).unwrap();
        view.adopt_tiles(&session);
        assert_eq!(listed(view.world()), vec![(1, 0), (2, 0)]);
        assert_eq!(view.world().global.raw_tiles, vec![(1, 0), (2, 0)]);
        assert_eq!(view.world().global.entry_points[0].group, 0);

        // one step back, and the view is back with the map
        session.undo().unwrap();
        view.adopt_tiles(&session);
        assert_eq!(listed(view.world()), vec![(0, 0), (1, 0), (2, 0)]);
        assert_eq!(view.world().global.entry_points[0].group, 1);
    }

    #[test]
    fn a_tile_added_is_in_the_list_and_a_tile_written_is_one_the_world_offers() {
        // The difference a save makes, which is why a tile appears on the ground a moment
        // after it appears in the list: a world streams tiles, and a tile with no file is not
        // a tile it can stream. (The map's own files, so that this is a test about the list
        // and the file rather than about which content roots a process happens to have.)
        let root = a_map_on_disk("written");
        let map_cfg = root.join("maps").join("Tiny").join("global.cfg");
        let mut session = Session::open(&map_cfg, omsi_editor_core::Destination::InPlace).unwrap();
        let mut view = a_view_over(host::open(&root, &map_cfg).unwrap());
        let offered = |view: &View| -> Vec<(i32, i32)> {
            view.world().select_tiles(Some((0, 0)), Some(4)).into_iter().map(|(x, y, _)| (x, y)).collect()
        };

        session.add_tile((2, 0)).unwrap();
        view.adopt_tiles(&session);
        assert_eq!(listed(view.world()), vec![(0, 0), (1, 0), (2, 0)], "the tile is not in the list");
        assert!(!offered(&view).contains(&(2, 0)), "a tile with no file was offered");

        session.save().unwrap();
        view.adopt_tiles(&session);
        assert!(offered(&view).contains(&(2, 0)), "the tile that was written is not offered: {:?}", offered(&view));
        assert!(root.join("maps").join("Tiny").join("tile_2_0.map").is_file());
    }
}
