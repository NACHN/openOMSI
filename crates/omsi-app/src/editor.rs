//! The object editor: a small part of what OMSI's map editor does, inside the game. The
//! scenery objects a tile places itself (its `[object]` records) can be picked, moved,
//! turned and deleted where they stand, and the tiles changed are written as copies into
//! the content folder - which the game reads before the installation - never into the
//! original map. New objects are made as copies of one that is there (C), and a copy
//! takes the shape of any other object of its folder (V): each is a new `[object]` record
//! after its model's. The ground is shaped with a brush where the view points (raised,
//! lowered, flattened); a tile's ground is written as its `.map.terrain` copy. Splines are
//! not part of it (the timetable is the launcher's Timetable page).
//!
//! # Frozen
//!
//! No feature is added here again. `openomsi-editor` - a program of its own, with a rail, a
//! dock, an undo stack and (in time) a plan view - is where map editing goes, and this was
//! the first attempt at it. Keep it working and keep fixing it when it breaks; it is what
//! `Ctrl+Shift+E` does today and what USER_GUIDE.md documents. But a capability added to the
//! standalone editor is **not** to be mirrored here, and the two front ends' middle layers
//! (this file's `Added`, its change list and its save loop, against `omsi-editor-core`'s
//! `Document`, `Session` and `NewObject`) are not to be merged: the plan is that this one
//! goes away once the standalone one can do everything it can. See `docs/EDITOR.md`, stage 0.
//!
//! Its one capability the standalone editor cannot match is editing against a world that is
//! being simulated - a bus at a stop, traffic, people - because the standalone one opens the
//! map without them. If that is wanted later it is an argument for a play-and-edit mode in
//! the standalone editor, not for keeping this file.
//!
//! Keys while it is on (Ctrl+Shift+E, or the game menu):
//! Enter picks the object nearest the middle of the view, Tab the next nearest;
//! I / K / J / L move it forward, back, left and right as the camera faces, U / O down and
//! up, N / M turn it (half a metre and five degrees a press, a tenth with Shift);
//! Delete takes it away (again: back), Backspace undoes all its edits, C copies it (the
//! copy is then the one edited), V gives a copy the next object of its folder, Ctrl+S
//! saves the changed tiles and Escape leaves the editor. Page Up / Page Down raise and
//! lower the ground under the middle of the view (a quarter metre, a twentieth with
//! Shift), F flattens it to the height at the middle, [ and ] make the brush smaller and
//! larger.

use crate::scene::{ObjectEdit, World};
use glam::{DVec3, Vec3};
use hashbrown::HashMap;
use omsi_editor_core::codec::{decode, encode};
use omsi_editor_core::ground::{aim, shape, GroundAction, BRUSH_DEFAULT};
use omsi_editor_core::{add_copies, clamp_brush, record_lines, rewrite_tile, NewRecord};
use std::path::{Path, PathBuf};

/// A new object: a copy of `template` (a map object), moved and turned from it, perhaps of
/// another type of the template's folder.
pub struct Added {
    pub template: i64,
    pub tile: (i32, i32),
    pub id: i64,
    /// Its `.sco` (the template's, or one of its folder: `V`).
    pub sco: PathBuf,
    pub base: DVec3,
    pub base_heading: f64,
    pub moved: DVec3,
    pub turned: f64,
    pub deleted: bool,
    gpu: Option<crate::scene::TileGpu>,
}

#[derive(Default)]
pub struct Editor {
    pub selected: Option<i64>,
    /// The copies made in this session, and the one being edited (it takes the keys).
    pub added: Vec<Added>,
    pub editing_added: Option<usize>,
    /// The tile of every object edited, by map id (its tile may be unloaded when saving).
    tiles: HashMap<i64, (i32, i32)>,
    /// The candidates of the last pick, nearest first (Tab walks them).
    candidates: Vec<i64>,
    next: usize,
    /// The ground brush's radius (m; 0: the default).
    brush: f64,
}

/// What a key does in the editor.
pub enum Action {
    Copy,
    Variant,
    Pick,
    NextPick,
    Move(DVec3),
    Turn(f64),
    Delete,
    Undo,
    Save,
    Leave,
    /// Raise (or lower) the ground under the view by metres.
    Ground(f64),
    Flatten,
    /// Make the brush larger (or smaller) by this factor.
    Brush(f64),
}

impl Editor {
    /// The objects in front of the camera, those nearest the middle of the view first.
    pub fn pick(&mut self, world: &World, scene: &omsi_render::Scene, eye: DVec3, forward: Vec3) -> Option<i64> {
        self.editing_added = None;
        // the picking itself is the world's, so a click chooses the same object here and in
        // the editor program (see `World::pick_candidates`)
        self.candidates = world.pick_candidates(scene, eye, forward, 150.0).into_iter().take(12).collect();
        self.next = 1;
        self.selected = self.candidates.first().copied();
        self.selected
    }

    pub fn next_pick(&mut self) -> Option<i64> {
        if self.candidates.is_empty() {
            return None;
        }
        let id = self.candidates[self.next % self.candidates.len()];
        self.next += 1;
        self.selected = Some(id);
        self.selected
    }

    /// The object being edited (a map object or a copy) put where the mouse drags it: its
    /// foot on the ground at `ground`, its turn kept.
    pub fn drag_to(&mut self, world: &World, renderer: &omsi_render::Renderer, scene: &mut omsi_render::Scene, ground: DVec3) -> Option<String> {
        if let Some(k) = self.editing_added {
            let a = self.added.get_mut(k)?;
            a.moved = ground - a.base;
            self.place_added(k, world, renderer, scene);
            return Some(self.describe(world));
        }
        let id = self.selected?;
        let (tile, pos) = world.edit_objects.lock().get(&id).map(|o| (o.tile, o.pos))?;
        let mut e = world.object_edits.lock().get(&id).copied().unwrap_or_default();
        e.moved = ground - pos;
        self.tiles.insert(id, tile);
        world.apply_object_edit(renderer, scene, id, e);
        Some(self.describe(world))
    }

    /// What the other players' games need to show the object being edited as it is now
    /// (`all`: every object edited or added this session): LAN commands, see
    /// `App::editor_broadcast`.
    pub fn sync_lines(&self, world: &World, root: &Path, all: bool) -> Vec<String> {
        let mut out = Vec::new();
        let edits = world.object_edits.lock();
        let line = |id: i64, e: &ObjectEdit| format!("objedit {id} {:.3} {:.3} {:.3} {:.2} {}", e.moved.x, e.moved.y, e.moved.z, e.turned, e.deleted as u8);
        if all {
            for (id, e) in edits.iter() {
                out.push(line(*id, e));
            }
        } else if let (None, Some(id)) = (self.editing_added, self.selected) {
            if let Some(e) = edits.get(&id) {
                out.push(line(id, e));
            }
        }
        let added_line = |a: &Added| {
            let rel = a.sco.strip_prefix(root).unwrap_or(&a.sco).to_string_lossy().replace('\\', "/");
            let p = a.base + a.moved;
            format!("objadd {} {:.2} {:.2} {:.2} {:.1} {} {rel}", a.id, p.x, p.y, p.z, a.base_heading + a.turned, a.deleted as u8)
        };
        if all {
            out.extend(self.added.iter().map(added_line));
        } else if let Some(a) = self.editing_added.and_then(|k| self.added.get(k)) {
            out.push(added_line(a));
        }
        out
    }

    /// Change the selected object; returns what to say.
    pub fn apply(&mut self, world: &World, renderer: &omsi_render::Renderer, scene: &mut omsi_render::Scene, action: &Action) -> Option<String> {
        match action {
            Action::Copy => return self.copy(world, renderer, scene),
            Action::Variant => return self.variant(world, renderer, scene),
            _ => {}
        }
        if let Some(k) = self.editing_added {
            let a = self.added.get_mut(k)?;
            match action {
                Action::Move(d) => a.moved += *d,
                Action::Turn(t) => a.turned += *t,
                Action::Delete => a.deleted = !a.deleted,
                Action::Undo => {
                    a.moved = DVec3::ZERO;
                    a.turned = 0.0;
                    a.deleted = false;
                }
                _ => return None,
            }
            self.place_added(k, world, renderer, scene);
            return Some(self.describe(world));
        }
        let id = self.selected?;
        let tile = world.edit_objects.lock().get(&id).map(|o| o.tile)?;
        let mut e = world.object_edits.lock().get(&id).copied().unwrap_or_default();
        match action {
            Action::Move(d) => e.moved += *d,
            Action::Turn(t) => e.turned += *t,
            Action::Delete => e.deleted = !e.deleted,
            Action::Undo => e = ObjectEdit::default(),
            _ => return None,
        }
        self.tiles.insert(id, tile);
        world.apply_object_edit(renderer, scene, id, e);
        Some(self.describe(world))
    }

    /// A copy of the selected object beside it, which the keys then move.
    fn copy(&mut self, world: &World, renderer: &omsi_render::Renderer, scene: &mut omsi_render::Scene) -> Option<String> {
        // a copy of a copy is a copy of its template
        let (template, sco, base, heading, tile) = match self.editing_added.and_then(|k| self.added.get(k)) {
            Some(a) => (a.template, a.sco.clone(), a.base + a.moved, a.base_heading + a.turned, a.tile),
            None => {
                let id = self.selected?;
                let objects = world.edit_objects.lock();
                let o = objects.get(&id)?;
                let e = world.object_edits.lock().get(&id).copied().unwrap_or_default();
                let f = o.xf.transform_vector3(Vec3::Y);
                let h = (f.x as f64).atan2(f.y as f64).to_degrees() + e.turned;
                (id, o.sco.clone(), o.pos + e.moved, h, o.tile)
            }
        };
        let max_id = world.edit_objects.lock().keys().copied().chain(self.added.iter().map(|a| a.id)).max().unwrap_or(0);
        let right = DVec3::new(heading.to_radians().cos(), -heading.to_radians().sin(), 0.0);
        self.added.push(Added { template, tile, id: max_id + 1, sco, base: base + right * 2.0, base_heading: heading, moved: DVec3::ZERO, turned: 0.0, deleted: false, gpu: None });
        let k = self.added.len() - 1;
        self.editing_added = Some(k);
        self.tiles.insert(template, tile);
        self.place_added(k, world, renderer, scene);
        Some(self.describe(world))
    }

    /// The copy being edited takes the next object type of its folder.
    fn variant(&mut self, world: &World, renderer: &omsi_render::Renderer, scene: &mut omsi_render::Scene) -> Option<String> {
        let k = self.editing_added?;
        let cur = self.added[k].sco.clone();
        let dir = cur.parent()?;
        let mut all: Vec<PathBuf> = std::fs::read_dir(dir).ok()?.flatten().map(|e| e.path()).filter(|p| p.extension().is_some_and(|e| e.eq_ignore_ascii_case("sco"))).collect();
        all.sort();
        let i = all.iter().position(|p| p == &cur).map(|i| (i + 1) % all.len()).unwrap_or(0);
        self.added[k].sco = all.get(i)?.clone();
        self.place_added(k, world, renderer, scene);
        Some(self.describe(world))
    }

    /// Draw copy `k` where it is now.
    fn place_added(&mut self, k: usize, world: &World, renderer: &omsi_render::Renderer, scene: &mut omsi_render::Scene) {
        let a = &mut self.added[k];
        if let Some(g) = a.gpu.take() {
            world.remove_helper_object(renderer, scene, g);
        }
        if !a.deleted {
            a.gpu = world.add_helper_object(renderer, scene, &a.sco.to_string_lossy(), a.base + a.moved, a.base_heading + a.turned, &[]);
        }
    }

    /// The selected object and what has been done to it.
    pub fn describe(&self, world: &World) -> String {
        if let Some(a) = self.editing_added.and_then(|k| self.added.get(k)) {
            let name = a.sco.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
            return if a.deleted { format!("New object {name}: taken away (Delete brings it back)") } else { format!("New object {} {name}: V for the next of its folder, C copies it again", a.id) };
        }
        let Some(id) = self.selected else { return "Object editor: Enter picks the object in the middle of the view".into() };
        let name = world
            .edit_objects
            .lock()
            .get(&id)
            .and_then(|o| o.sco.file_name().map(|n| n.to_string_lossy().to_string()))
            .unwrap_or_default();
        let e = world.object_edits.lock().get(&id).copied().unwrap_or_default();
        if e.deleted {
            format!("Object {id} {name}: deleted (Delete brings it back)")
        } else if e == ObjectEdit::default() {
            format!("Object {id} {name}")
        } else {
            format!("Object {id} {name}: moved {:+.2} / {:+.2} / {:+.2} m, turned {:+.1}°", e.moved.x, e.moved.y, e.moved.z, e.turned)
        }
    }

    /// Where the middle of the view meets the ground (within 400 m): the kernel's own ray walk,
    /// the one the standalone editor aims its brush with.
    pub fn aim(world: &World, eye: DVec3, forward: Vec3) -> Option<DVec3> {
        aim(|x, y| world.ground_terrain(x, y), eye, forward.as_dvec3(), 400.0)
    }

    /// The brush's radius now.
    pub fn brush(&self) -> f64 {
        clamp_brush(if self.brush > 0.0 { self.brush } else { BRUSH_DEFAULT })
    }

    /// Shape the ground about `at` (raise by `Ground`'s metres, or flatten to the height at
    /// `at`); `Brush` resizes the brush. Returns what to say and the tiles to read again.
    pub fn ground(&mut self, world: &World, at: Option<DVec3>, action: &Action) -> (String, Vec<(i32, i32)>) {
        if let Action::Brush(f) = action {
            self.brush = clamp_brush(self.brush() * f);
            return (format!("Ground brush: {:.1} m", self.brush), Vec::new());
        }
        let Some(at) = at else { return ("Point the view at the ground".into(), Vec::new()) };
        let r = self.brush();
        let size = omsi_map::tile_size();
        let target = world.ground_terrain(at.x, at.y).unwrap_or(at.z);
        let mut changed = Vec::new();
        let mut edits = world.terrain_edits.lock();
        let (tx0, tx1) = (((at.x - r) / size).floor() as i32, ((at.x + r) / size).floor() as i32);
        let (ty0, ty1) = (((at.y - r) / size).floor() as i32, ((at.y + r) / size).floor() as i32);
        for tx in tx0..=tx1 {
            for ty in ty0..=ty1 {
                if !edits.contains_key(&(tx, ty)) {
                    let Some(src) = world.tile_source(tx, ty) else { continue };
                    let t = omsi_map::Terrain::load(&crate::scene::tile_companion(&src, ".terrain")).unwrap_or_else(|_| omsi_map::Terrain::flat());
                    edits.insert((tx, ty), t);
                }
                let t = edits.get_mut(&(tx, ty)).unwrap();
                // the same brush the standalone editor runs, on this tile's ground
                let brush_action = match action {
                    Action::Ground(m) => GroundAction::Raise(*m),
                    _ => GroundAction::Flatten(target),
                };
                if shape(t, (tx as f64 * size, ty as f64 * size), at, r, brush_action).is_some() {
                    changed.push((tx, ty));
                }
            }
        }
        let what = match action {
            Action::Ground(d) if *d > 0.0 => format!("Ground raised {:.2} m", d),
            Action::Ground(d) => format!("Ground lowered {:.2} m", -d),
            _ => format!("Ground flattened to {:.2} m", target),
        };
        (format!("{what} (brush {:.1} m, {} tile(s)) - Ctrl+S saves", r, changed.len()), changed)
    }

    /// Write every tile with edits as a copy under `content` (the map's own folder there),
    /// from the file the game reads it from. Returns the files written.
    ///
    /// The record rewriting is `omsi-editor-core`'s, the same code the standalone editor runs,
    /// so a copy made here and a copy made there are written the same way. In particular a copy
    /// carries its template's own record lines with it (taken from the text as it was read), so
    /// it survives the same save taking the template away.
    pub fn save(&self, world: &World, map_rel: &str, content: &Path, original: &Path) -> Result<Vec<PathBuf>, String> {
        let edits = world.object_edits.lock().clone();
        let mut by_tile: HashMap<(i32, i32), HashMap<i64, ObjectEdit>> = HashMap::new();
        for (id, e) in edits {
            let Some(tile) = self.tiles.get(&id) else { continue };
            by_tile.entry(*tile).or_default().insert(id, e);
        }
        // every tile this save touches: the ones an object was edited in, and the ones a copy
        // goes into. Each file is read and decoded once, and the same text is used both to take
        // a copy's template record and to be written back.
        let mut touched: Vec<(i32, i32)> = by_tile.keys().copied().collect();
        for a in self.added.iter().filter(|a| !a.deleted) {
            if !touched.contains(&a.tile) {
                touched.push(a.tile);
            }
        }
        let mut sources: HashMap<(i32, i32), (PathBuf, String, _)> = HashMap::new();
        for id in &touched {
            let src = world.tile_source(id.0, id.1).ok_or_else(|| format!("tile ({}, {}) is not in the map", id.0, id.1))?;
            let bytes = omsi_cfg::vfs::read(&src).map_err(|e| format!("{}: {e}", src.display()))?;
            let (text, enc) = decode(&bytes);
            sources.insert(*id, (src, text, enc));
        }
        let mut copies_by_tile: HashMap<(i32, i32), Vec<NewRecord>> = HashMap::new();
        for a in self.added.iter().filter(|a| !a.deleted) {
            let Some((_, text, _)) = sources.get(&a.tile) else { continue };
            let file = a.sco.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
            // measured against where the template's record puts it *on disk* - the record a copy
            // is written from is the one the file has, not where the template has since moved to
            let template_pos = world.edit_objects.lock().get(&a.template).map(|o| o.pos).unwrap_or(a.base);
            let offset = a.base + a.moved - template_pos;
            let turned = a.base_heading + a.turned - template_heading(world, a.template).unwrap_or(a.base_heading);
            copies_by_tile.entry(a.tile).or_default().push(NewRecord {
                template: a.template,
                id: a.id,
                file,
                moved: offset,
                turned,
                template_lines: record_lines(text, a.template).unwrap_or_default(),
            });
            by_tile.entry(a.tile).or_default();
        }
        let map_dir = Path::new(map_rel).parent().unwrap_or(Path::new(""));
        let mut written = Vec::new();
        for ((tx, ty), edits) in by_tile {
            let (src, text, enc) = &sources[&(tx, ty)];
            let copies = copies_by_tile.get(&(tx, ty)).map(|v| v.as_slice()).unwrap_or(&[]);
            if edits.is_empty() && copies.is_empty() {
                continue;
            }
            let name = src.file_name().ok_or("tile without a name")?.to_owned();
            let out = content.join(map_dir).join(&name);
            // never into the installation itself
            if let (Ok(o), Ok(r)) = (out.parent().map(|p| p.to_path_buf()).unwrap_or_default().canonicalize().or_else(|_| Ok::<_, std::io::Error>(out.clone())), original.canonicalize()) {
                if o.starts_with(&r) {
                    return Err(format!("{} lies in the original installation: not written", out.display()));
                }
            }
            let (new_text, n) = rewrite_tile(text, &edits);
            let (new_text, c) = add_copies(&new_text, copies);
            let n = n + c;
            if n == 0 {
                return Err(format!("the objects edited were not found in {}", src.display()));
            }
            std::fs::create_dir_all(out.parent().unwrap_or(Path::new("."))).map_err(|e| e.to_string())?;
            let data = encode(&new_text, *enc);
            std::fs::write(&out, data).map_err(|e| format!("{}: {e}", out.display()))?;
            log::info!("object editor: {} objects of tile ({tx}, {ty}) changed, written to {}", n, out.display());
            written.push(out);
        }
        // the ground the brush shaped: each tile's .map.terrain
        let terrains = world.terrain_edits.lock().clone();
        for ((tx, ty), t) in terrains {
            let src = world.tile_source(tx, ty).ok_or_else(|| format!("tile ({tx}, {ty}) is not in the map"))?;
            let name = format!("{}.terrain", src.file_name().ok_or("tile without a name")?.to_string_lossy());
            let out = content.join(map_dir).join(&name);
            if let (Some(dir), Ok(r)) = (out.parent(), original.canonicalize()) {
                if dir.canonicalize().unwrap_or_else(|_| dir.to_path_buf()).starts_with(&r) {
                    return Err(format!("{} lies in the original installation: not written", out.display()));
                }
            }
            std::fs::create_dir_all(out.parent().unwrap_or(Path::new("."))).map_err(|e| e.to_string())?;
            std::fs::write(&out, t.to_bytes()).map_err(|e| format!("{}: {e}", out.display()))?;
            log::info!("map editor: the ground of tile ({tx}, {ty}) written to {}", out.display());
            written.push(out);
        }
        Ok(written)
    }
}

/// The heading map object `id` has *on disk* - its tile record's own heading, not counting
/// anything done to it this session.
///
/// A copy's record is its template's record with a delta added ([`omsi_editor_core::NewRecord`]),
/// so the delta has to be measured against the record's heading. Adding this session's turn here
/// would eat the template's own turn: a template turned 30° and then copied would be drawn at
/// 30° but written at 0°.
fn template_heading(world: &World, id: i64) -> Option<f64> {
    let objects = world.edit_objects.lock();
    let o = objects.get(&id)?;
    let f = o.xf.transform_vector3(Vec3::Y);
    Some((f.x as f64).atan2(f.y as f64).to_degrees())
}

/// The editor's key for `code` (with Shift for the fine steps), as the camera faces `yaw`
/// (degrees clockwise from north).
pub fn action_for(code: winit::keyboard::KeyCode, shift: bool, ctrl: bool, yaw: f64) -> Option<Action> {
    use winit::keyboard::KeyCode as K;
    let step = if shift { 0.05 } else { 0.5 };
    let turn = if shift { 0.5 } else { 5.0 };
    let (s, c) = yaw.to_radians().sin_cos();
    let fwd = DVec3::new(s, c, 0.0) * step;
    let right = DVec3::new(c, -s, 0.0) * step;
    Some(match code {
        K::Enter | K::NumpadEnter => Action::Pick,
        K::KeyC if !ctrl => Action::Copy,
        K::KeyV if !ctrl => Action::Variant,
        K::Tab => Action::NextPick,
        K::KeyI => Action::Move(fwd),
        K::KeyK => Action::Move(-fwd),
        K::KeyL => Action::Move(right),
        K::KeyJ => Action::Move(-right),
        K::KeyO => Action::Move(DVec3::Z * step),
        K::KeyU => Action::Move(-DVec3::Z * step),
        K::KeyM => Action::Turn(turn),
        K::KeyN => Action::Turn(-turn),
        K::Delete => Action::Delete,
        K::Backspace => Action::Undo,
        K::KeyS if ctrl => Action::Save,
        K::PageUp => Action::Ground(if shift { 0.05 } else { 0.25 }),
        K::PageDown => Action::Ground(if shift { -0.05 } else { -0.25 }),
        K::KeyF if !ctrl => Action::Flatten,
        K::BracketRight => Action::Brush(1.25),
        K::BracketLeft => Action::Brush(0.8),
        K::Escape => Action::Leave,
        _ => return None,
    })
}
