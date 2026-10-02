//! The open map: its tiles, its object index, what has been changed, and how it is written
//! back.
//!
//! A [`Document`] is the whole of an editing session's state and has no idea a window exists.
//! `omsi-editor` drives it from a camera and panels; `omsi-app`'s in-game editor drives it
//! from keys. Both get the same behaviour, which is the point of it living here.
//!
//! # What is indexed
//!
//! Opening reads every tile file once and records each `[object]` - its id, its `.sco`, where
//! it stands. That index is what makes an object addressable by id across the whole map, and
//! what a placed copy is measured against. `[attachObj]` records are left alone: an attached
//! object hangs on a parent, and moving it by itself would break the attachment.
//!
//! # Two texts, one file
//!
//! The text read at open is kept as it was (`TileDoc::text`), and a save always rewrites
//! *that* text with the current edits. It never re-reads the file it wrote last time. This
//! matters: re-reading would apply a move a second time, so saving twice in a row would move
//! an object twice. Keeping the original text makes saving idempotent, and it is what lets
//! `Destination::Content` copy a tile and still be safe to save again and again.
//!
//! # Where a save goes
//!
//! [`Destination::Content`] writes copies under the content folder, mirroring the map's place
//! in it; the game reads those before the installation, so the original is never touched. This
//! is what the game itself does and it is the default.
//!
//! [`Destination::InPlace`] writes the map's own files - what someone editing *their* map
//! wants. The first time this session changes a file, its previous contents are copied to
//! `<file>.openomsi-bak`; after that the snapshot is left alone, so it holds the state the
//! session started from rather than the state before the previous keystroke.

use crate::codec::{decode, encode, Encoding};
use crate::ground::{self, GroundAction, GridRect};
use crate::record::{add_copies, object_records, rewrite_tile, NewRecord, ObjectEdit};
use crate::EditError;
use glam::DVec3;
use hashbrown::{HashMap, HashSet};
use omsi_map::{GlobalCfg, MapTileRef, Terrain};
use std::path::{Path, PathBuf};

/// A tile's place in the map, as `global.cfg` lists it.
pub type TileId = (i32, i32);

/// Where a save goes.
#[derive(Clone, Debug)]
pub enum Destination {
    /// Copies under this folder, keeping the map's place in the installation's layout.
    Content(PathBuf),
    /// The map's own files, with a `<file>.openomsi-bak` snapshot taken before the first
    /// change to each.
    InPlace,
}

/// The suffix a snapshot gets.
pub const BACKUP_SUFFIX: &str = ".openomsi-bak";

/// Where a tile's own corner is on the map, in metres.
///
/// A tile file's object coordinates start at its own corner, so this is what turns "46 m
/// across and 12 m up the tile" into a place on the map. The map's own frame is the one
/// everything else in the editor works in - the crosshair, an entry point, the camera - and
/// two spaces that look alike are how an object ends up in the next field.
pub fn tile_origin(tile: TileId) -> DVec3 {
    let size = omsi_map::tile_size();
    DVec3::new(tile.0 as f64 * size, tile.1 as f64 * size, 0.0)
}

/// An object of the map, as its tile file has it (before any edit).
#[derive(Clone, Debug)]
pub struct ObjectRef {
    pub tile: TileId,
    /// Where the record puts it.
    pub pos: DVec3,
    /// Its heading, degrees clockwise (the record's first rotation).
    pub heading: f64,
    /// Its `.sco`, as the record writes it (`Sceneryobjects\…\x.sco`).
    pub file: String,
}

impl ObjectRef {
    /// The folder the `.sco` lives in - what a copy's variant is looked for in.
    pub fn folder(&self) -> PathBuf {
        Path::new(&self.file.replace('/', "\\")).parent().map(|p| p.to_path_buf()).unwrap_or_default()
    }

    /// Just the file name.
    pub fn file_name(&self) -> String {
        file_name_of(&self.file)
    }
}

/// An object placed in this session: a copy of `template` put in `tile`, which is what the
/// tile file gains on save.
#[derive(Clone, Debug, PartialEq)]
pub struct NewObject {
    /// The map object it was copied from - where its folder and its record's place come from.
    pub template: Option<i64>,
    pub tile: TileId,
    pub id: i64,
    /// The file name the record gets, inside the template's folder.
    pub file: String,
    /// The template's place when the copy was made, and the copy's own from there.
    pub base: DVec3,
    pub base_heading: f64,
    pub moved: DVec3,
    pub turned: f64,
    /// Taken away again in this session. The copy is still here - putting it back is the
    /// same edit the other way - but nothing of it is drawn or written.
    pub deleted: bool,
    /// The copy's `.sco` as a full record path, backslashes and all - what is drawn.
    pub sco: String,
    /// The template's own record lines, as its tile file has them, line endings included.
    ///
    /// The copy is written by rewriting these, so it does not depend on the template's record
    /// still being in the file at save time - which it is not when the same save takes the
    /// template away. Taken when the copy is placed; see [`Document::object_record`].
    pub template_record: Vec<String>,
}

impl NewObject {
    pub fn position(&self) -> DVec3 {
        self.base + self.moved
    }

    pub fn heading(&self) -> f64 {
        self.base_heading + self.turned
    }

    /// The record this copy adds to its tile, measured against the template's own record as it
    /// stands *on disk* - not against where the template has since been moved to.
    pub fn record(&self, template_pos: DVec3, template_heading: f64) -> NewRecord {
        NewRecord {
            template: self.template.unwrap_or(self.id),
            id: self.id,
            file: file_name_of(&self.sco),
            moved: self.position() - template_pos,
            turned: self.heading() - template_heading,
            template_lines: self.template_record.clone(),
        }
    }
}

/// Just the file name of a `.sco` written as a record writes it.
pub fn file_name_of(sco: &str) -> String {
    Path::new(&sco.replace('/', "\\"))
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_default()
}

/// The folder part of a `.sco` written as a record writes it (``Sceneryobjects\A\``), its
/// separator included; empty when the record names no folder.
pub fn folder_of(sco: &str) -> String {
    match sco.rfind(['\\', '/']) {
        Some(p) => sco[..=p].to_string(),
        None => String::new(),
    }
}

/// One tile file, open for editing: its text as it was read, and the encoding to put it back.
#[derive(Clone, Debug)]
pub struct TileDoc {
    pub source: PathBuf,
    /// The text exactly as the file had it - what every save rewrites.
    pub text: String,
    pub encoding: Encoding,
}

/// What a save wrote.
#[derive(Clone, Debug, Default)]
pub struct SaveReport {
    /// The files written.
    pub files: Vec<PathBuf>,
    /// The snapshots taken before an in-place write.
    pub backups: Vec<PathBuf>,
    /// How many object records changed or were added.
    pub objects: usize,
    /// How many tiles had their ground written.
    pub grounded: usize,
}

impl SaveReport {
    pub fn is_empty(&self) -> bool {
        self.files.is_empty()
    }

    /// The one-line summary a status bar shows.
    pub fn describe(&self) -> String {
        if self.is_empty() {
            return "Nothing to save".into();
        }
        format!(
            "Saved: {} object(s), {} tile ground(s), {} file(s){}",
            self.objects,
            self.grounded,
            self.files.len(),
            if self.backups.is_empty() {
                String::new()
            } else {
                format!(" and {} snapshot(s)", self.backups.len())
            }
        )
    }
}

/// A hash of a tile's ground, so "has it changed?" is a comparison and not a flag some later
/// undo would have to remember to clear. Terrain files are ~15 kB; hashing one is nothing
/// next to writing it, and it happens only while something is already dirty.
fn terrain_hash(t: &Terrain) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for v in &t.heights {
        for b in v.to_bits().to_le_bytes() {
            h ^= b as u64;
            h = h.wrapping_mul(0x0000_0100_0000_01b3);
        }
    }
    h ^ (t.cells as u64)
}

/// An open map.
pub struct Document {
    map_cfg: PathBuf,
    map_dir: PathBuf,
    /// The installation root the map lives in - what a content-folder copy's place is measured
    /// from.
    install_root: PathBuf,
    destination: Destination,
    global: GlobalCfg,
    tiles: HashMap<TileId, MapTileRef>,
    loaded: HashMap<TileId, TileDoc>,
    objects: HashMap<i64, ObjectRef>,
    edits: HashMap<i64, ObjectEdit>,
    added: HashMap<i64, NewObject>,
    terrain: HashMap<TileId, Terrain>,
    /// The ground of each read tile as it was at the last save, so "did the ground change?" is
    /// answered by comparison. This is what lets undoing a brush stroke make the session clean
    /// again instead of leaving a flag behind.
    terrain_hash: HashMap<TileId, u64>,
    next_id: i64,
    /// Files already snapshotted in this session (in-place writing only).
    snapshotted: HashSet<PathBuf>,
    /// Bumped every time a save moves the baseline (see [`Document::revision`]).
    revision: u64,
}

impl Document {
    /// Open the map `map_cfg` (`…/maps/<name>/global.cfg`) and index every object it holds.
    pub fn open(map_cfg: impl AsRef<Path>, destination: Destination) -> Result<Self, EditError> {
        let map_cfg = map_cfg.as_ref().to_path_buf();
        let global = GlobalCfg::load(&map_cfg).map_err(|e| EditError::Cfg { path: map_cfg.clone(), source: e })?;
        let map_dir = global.dir().to_path_buf();
        let install_root = map_dir.parent().and_then(|p| p.parent()).map(|p| p.to_path_buf()).unwrap_or_else(|| map_dir.clone());
        let tiles: HashMap<TileId, MapTileRef> = global.tiles.iter().map(|t| ((t.x, t.y), t.clone())).collect();
        let mut doc = Document {
            map_cfg,
            map_dir,
            install_root,
            destination,
            global,
            tiles,
            loaded: HashMap::new(),
            objects: HashMap::new(),
            edits: HashMap::new(),
            added: HashMap::new(),
            terrain: HashMap::new(),
            terrain_hash: HashMap::new(),
            next_id: 1,
            snapshotted: HashSet::new(),
            revision: 0,
        };
        doc.index()?;
        Ok(doc)
    }

    /// Open a map that is edited where it lies (`Destination::InPlace`).
    pub fn open_in_place(map_cfg: impl AsRef<Path>) -> Result<Self, EditError> {
        Document::open(map_cfg, Destination::InPlace)
    }

    /// Read every tile and index its `[object]` records. An unreadable tile is skipped with a
    /// warning: one broken file must not stop a map from opening.
    fn index(&mut self) -> Result<(), EditError> {
        let mut objects: HashMap<i64, ObjectRef> = HashMap::new();
        for (id, tile) in self.tiles.iter().map(|(k, v)| (*k, v.clone())).collect::<Vec<_>>() {
            let src = omsi_cfg::resolve_path(&self.map_dir, &tile.file);
            let bytes = match omsi_cfg::vfs::read(&src) {
                Ok(b) => b,
                Err(e) => {
                    log::warn!("{}: {e}", src.display());
                    continue;
                }
            };
            let (text, encoding) = decode(&bytes);
            let origin = tile_origin(id);
            for r in object_records(&text) {
                // The record's coordinates are *tile-local*: that is what the tile file
                // writes and what OMSI reads them as, against the tile's own place on the
                // map. Everything above this line wants them where they actually are - a
                // move to a place the view picked, a copy measured from its template, the
                // place the editor draws a selection on - so the tile's own place is added
                // here and never thought about again. An edit is a difference and does not
                // care which space it is in, which is why the file is still written by
                // adding it to the coordinates the file already holds.
                objects.entry(r.id).or_insert_with(|| ObjectRef { tile: id, pos: origin + r.pos, heading: r.heading, file: r.file });
            }
            self.loaded.insert(id, TileDoc { source: src, text, encoding });
        }
        self.objects = objects;
        // never hand out an id the map already uses
        let highest = self.objects.keys().copied().max().unwrap_or(0);
        self.next_id = self.global.next_id_code.max(highest + 1).max(1);
        // a copy already on disk (a previous session's content folder) is part of the map too
        Ok(())
    }

    // ---- reading ---------------------------------------------------------------------

    pub fn map_cfg(&self) -> &Path {
        &self.map_cfg
    }

    pub fn map_dir(&self) -> &Path {
        &self.map_dir
    }

    pub fn destination(&self) -> &Destination {
        &self.destination
    }

    pub fn global(&self) -> &GlobalCfg {
        &self.global
    }

    pub fn tile_count(&self) -> usize {
        self.tiles.len()
    }

    /// How many tiles could actually be read.
    pub fn loaded_tiles(&self) -> usize {
        self.loaded.len()
    }

    pub fn object_count(&self) -> usize {
        self.objects.len()
    }

    /// Every object of the map, by id.
    pub fn objects(&self) -> impl Iterator<Item = (i64, &ObjectRef)> {
        self.objects.iter().map(|(k, v)| (*k, v))
    }

    pub fn object(&self, id: i64) -> Option<&ObjectRef> {
        self.objects.get(&id)
    }

    /// The objects of one tile.
    pub fn objects_in(&self, tile: TileId) -> impl Iterator<Item = (i64, &ObjectRef)> {
        self.objects.iter().filter(move |(_, o)| o.tile == tile).map(|(k, v)| (*k, v))
    }

    pub fn tiles(&self) -> impl Iterator<Item = TileId> + '_ {
        self.tiles.keys().copied()
    }

    /// What has been done to object `id` (nothing, if it is untouched).
    ///
    /// A copy placed this session is answered for out of the [`NewObject`] itself: it has no
    /// record in the tile's text for an edit to sit beside yet, and the copy's own
    /// `moved`/`turned`/`deleted` *are* what has been done to it. Reading and writing both go
    /// through here, so a move of a fresh copy and a move of a map object are the same thing
    /// to every caller - and a save writes both.
    pub fn edit(&self, id: i64) -> ObjectEdit {
        if let Some(a) = self.added.get(&id) {
            return ObjectEdit { moved: a.moved, turned: a.turned, deleted: a.deleted };
        }
        self.edits.get(&id).copied().unwrap_or_default()
    }

    /// Where object `id` stands now: its record, moved.
    pub fn position(&self, id: i64) -> Option<DVec3> {
        if let Some(a) = self.added.get(&id) {
            return Some(a.position());
        }
        let o = self.objects.get(&id)?;
        Some(o.pos + self.edit(id).moved)
    }

    /// The copies placed in this session that are still standing, in id order.
    pub fn added(&self) -> impl Iterator<Item = &NewObject> {
        let mut v: Vec<&NewObject> = self.added.values().filter(|a| !a.deleted).collect();
        v.sort_by_key(|a| a.id);
        v.into_iter()
    }

    /// Every copy placed this session, taken away ones included - what "put it back" needs.
    pub fn added_object(&self, id: i64) -> Option<&NewObject> {
        self.added.get(&id)
    }

    /// How many copies placed this session are standing.
    pub fn added_count(&self) -> usize {
        self.added.values().filter(|a| !a.deleted).count()
    }

    /// The id the next placed object gets.
    pub fn next_id(&self) -> i64 {
        self.next_id
    }

    /// Whether anything has been changed since the last save. Computed, never remembered: an
    /// undo that puts everything back reports clean, as it should.
    pub fn is_dirty(&self) -> bool {
        !self.dirty_tiles().is_empty()
    }

    /// The tiles a save would write.
    pub fn dirty_tiles(&self) -> Vec<TileId> {
        let mut out: Vec<TileId> = Vec::new();
        for (tile, _) in self.terrain_hash.iter() {
            if self.ground_is_dirty(*tile) {
                out.push(*tile);
            }
        }
        for (id, e) in self.edits.iter() {
            if e.is_untouched() {
                continue;
            }
            if let Some(o) = self.objects.get(id) {
                out.push(o.tile);
            }
        }
        for a in self.added.values() {
            out.push(a.tile);
        }
        out.sort_unstable();
        out.dedup();
        out
    }

    /// Whether a tile's ground has been shaped since the last save.
    pub fn ground_is_dirty(&self, id: TileId) -> bool {
        match (self.terrain.get(&id), self.terrain_hash.get(&id)) {
            (Some(now), Some(then)) => terrain_hash(now) != *then,
            _ => false,
        }
    }

    /// Where the tile `id` is read from: a copy in the content folder when there is one (the
    /// game reads that first), otherwise the installation's own file.
    pub fn tile_source(&self, id: TileId) -> Option<PathBuf> {
        let t = self.tiles.get(&id)?;
        let original = omsi_cfg::resolve_path(&self.map_dir, &t.file);
        match &self.destination {
            Destination::InPlace => Some(original),
            Destination::Content(root) => {
                let copy = self.content_path(root, &original);
                Some(if copy.exists() { copy } else { original })
            }
        }
    }

    /// Where a save would put a file that is read from `src`.
    fn target_of(&self, src: &Path) -> PathBuf {
        match &self.destination {
            Destination::InPlace => src.to_path_buf(),
            Destination::Content(root) => self.content_path(root, src),
        }
    }

    fn content_path(&self, root: &Path, src: &Path) -> PathBuf {
        let rel = src.strip_prefix(&self.install_root).unwrap_or_else(|_| src.file_name().map(Path::new).unwrap_or(Path::new("")));
        root.join(rel)
    }

    /// Read a tile's text as it was opened (for a *View source* panel).
    pub fn tile_text(&self, id: TileId) -> Option<&str> {
        self.loaded.get(&id).map(|t| t.text.as_str())
    }

    /// An object's `[object]` record: its lines exactly as its tile file has them, endings
    /// included. This is what a copy of it is written from, so that the copy keeps whatever
    /// the record carries that this project does not parse - a bus stop's labels, a tree's
    /// parameters, a mod's own keywords.
    pub fn object_record(&self, id: i64) -> Option<Vec<String>> {
        let tile = self.objects.get(&id)?.tile;
        let doc = self.loaded.get(&tile)?;
        crate::record::record_lines(&doc.text, id)
    }

    /// The height of the ground at a world position, from the edited ground when there is one.
    pub fn ground_height(&mut self, x: f64, y: f64) -> Option<f64> {
        let (id, (lx, ly)) = omsi_map::world_to_tile_local(x, y);
        if !self.tiles.contains_key(&id) {
            return None;
        }
        let t = self.terrain_for(id).ok()?;
        Some(t.sample(lx as f32, ly as f32) as f64)
    }

    /// The ground of a tile, read from its `.map.terrain` companion the first time.
    pub fn terrain_for(&mut self, id: TileId) -> Result<&mut Terrain, EditError> {
        if !self.terrain.contains_key(&id) {
            let t = self
                .tile_source(id)
                .map(|src| companion(&src))
                .and_then(|p| Terrain::load(&p).ok())
                .unwrap_or_else(Terrain::flat);
            self.terrain_hash.insert(id, terrain_hash(&t));
            self.terrain.insert(id, t);
        }
        Ok(self.terrain.get_mut(&id).expect("just inserted"))
    }

    /// The ground of a tile without changing anything (for a preview or a readout).
    pub fn terrain(&self, id: TileId) -> Option<&Terrain> {
        self.terrain.get(&id)
    }

    pub fn edited_terrain_tiles(&self) -> usize {
        self.terrain.len()
    }

    /// Every object this session has changed, with what was done to it.
    ///
    /// A window has to put these on the screen and cannot ask once per object: a map holds
    /// tens of thousands, and only the handful that were touched are of any interest. Objects
    /// whose edit was taken back are not here - there is nothing to show for them.
    pub fn edits(&self) -> impl Iterator<Item = (i64, ObjectEdit)> + '_ {
        self.edits.iter().map(|(id, e)| (*id, *e))
    }

    /// The tiles whose ground has been changed since the last save, with the ground as it
    /// stands - what a save would write, and what a window has to draw again.
    pub fn changed_ground(&self) -> impl Iterator<Item = (TileId, &Terrain)> + '_ {
        self.terrain.iter().filter(|(id, _)| self.ground_is_dirty(**id)).map(|(id, t)| (*id, t))
    }

    // ---- writing (the commands in `command.rs` are the only callers) -------------------

    /// The tile an object belongs to, whether it is the map's or one placed here.
    fn tile_of(&self, id: i64) -> Option<TileId> {
        self.objects.get(&id).map(|o| o.tile).or_else(|| self.added.get(&id).map(|a| a.tile))
    }

    pub(crate) fn set_edit(&mut self, id: i64, edit: ObjectEdit) -> Result<(), EditError> {
        // a copy placed this session: what an edit is *for it* is where its record will go,
        // so it lands on the copy itself rather than in the table the map's objects use
        if let Some(a) = self.added.get_mut(&id) {
            a.moved = edit.moved;
            a.turned = edit.turned;
            a.deleted = edit.deleted;
            return Ok(());
        }
        if self.tile_of(id).is_none() {
            return Err(EditError::NoObject(id));
        }
        if edit.is_untouched() {
            self.edits.remove(&id);
        } else {
            self.edits.insert(id, edit);
        }
        Ok(())
    }

    pub(crate) fn place(&mut self, obj: NewObject) {
        if obj.id >= self.next_id {
            self.next_id = obj.id + 1;
        }
        self.added.insert(obj.id, obj);
    }

    pub(crate) fn take_away(&mut self, id: i64) -> Option<NewObject> {
        self.added.remove(&id)
    }

    /// Give a placed object another `.sco`; returns the one it had.
    pub(crate) fn set_added_file(&mut self, id: i64, sco: String) -> Result<String, EditError> {
        let a = self.added.get_mut(&id).ok_or(EditError::NoObject(id))?;
        let old = std::mem::replace(&mut a.sco, sco);
        a.file = file_name_of(&a.sco);
        Ok(old)
    }

    /// Shape the ground under `at`: the tile it falls in, and the samples that moved.
    pub(crate) fn shape_ground(&mut self, at: DVec3, radius: f64, action: GroundAction) -> Result<Option<(TileId, GridRect, Vec<f32>)>, EditError> {
        let (id, (lx, ly)) = omsi_map::world_to_tile_local(at.x, at.y);
        if !self.tiles.contains_key(&id) {
            return Ok(None);
        }
        // the tile's origin in world metres, whatever the tile size is
        let origin = (at.x - lx, at.y - ly);
        let t = self.terrain_for(id)?;
        let Some((rect, before)) = ground::shape(t, origin, at, radius, action) else {
            return Ok(None);
        };
        Ok(Some((id, rect, before)))
    }

    /// Put a brush stroke back.
    pub(crate) fn restore_ground(&mut self, id: TileId, rect: &GridRect, values: &[f32]) -> Result<(), EditError> {
        let t = self.terrain_for(id)?;
        ground::restore(t, rect, values);
        Ok(())
    }

    /// The samples of a brush-stroke rectangle as they are now (to undo a restore).
    pub(crate) fn capture_ground(&mut self, id: TileId, rect: &GridRect) -> Result<Vec<f32>, EditError> {
        let t = self.terrain_for(id)?;
        Ok(ground::capture(t, rect))
    }

    // ---- saving ----------------------------------------------------------------------

    /// Write everything changed since the last save. Nothing is written when nothing changed.
    ///
    /// A save that writes anything **moves the baseline**: the edits it wrote are folded into
    /// the object index and forgotten, the objects placed this session become ordinary map
    /// objects, the text it wrote becomes the text the next save rewrites, and
    /// [`Document::revision`] goes up. Everything read from the document afterwards describes
    /// the map as it now is on disk.
    pub fn save(&mut self) -> Result<SaveReport, EditError> {
        let mut report = SaveReport::default();
        for tile in self.dirty_tiles() {
            self.save_tile(tile, &mut report)?;
        }
        if !report.is_empty() {
            self.revision += 1;
        }
        Ok(report)
    }

    /// How many times a save has moved the baseline. A [`History`](crate::History) refuses to
    /// take back a step from before the last one: those steps describe edits against a
    /// baseline that no longer exists, and applying them again would be wrong rather than
    /// merely useless.
    pub fn revision(&self) -> u64 {
        self.revision
    }

    /// Fold the edits a save just wrote into the index, and make the text it wrote the text
    /// the next save starts from.
    fn rebaseline(&mut self, tile: TileId, text: String) {
        let edited: Vec<i64> = self
            .edits
            .iter()
            .filter(|(&id, e)| !e.is_untouched() && self.objects.get(&id).is_some_and(|o| o.tile == tile))
            .map(|(k, _)| *k)
            .collect();
        for id in edited {
            let Some(e) = self.edits.remove(&id) else { continue };
            if e.deleted {
                // its record is gone from the file, so it is gone from the map
                self.objects.remove(&id);
            } else if let Some(o) = self.objects.get_mut(&id) {
                o.pos += e.moved;
                o.heading += e.turned;
            }
        }
        let placed: Vec<i64> = self.added.values().filter(|a| a.tile == tile).map(|a| a.id).collect();
        for id in placed {
            let Some(a) = self.added.remove(&id) else { continue };
            // a copy taken away was never written: it is gone from the map rather than
            // becoming an object the next open would find
            if a.deleted {
                continue;
            }
            self.objects.insert(
                id,
                ObjectRef { tile, pos: a.position(), heading: a.heading(), file: a.sco.clone() },
            );
        }
        if let Some(d) = self.loaded.get_mut(&tile) {
            d.text = text;
        }
    }

    fn save_tile(&mut self, tile: TileId, report: &mut SaveReport) -> Result<(), EditError> {
        let edits: HashMap<i64, ObjectEdit> = self
            .edits
            .iter()
            .filter(|(&id, e)| !e.is_untouched() && self.objects.get(&id).is_some_and(|o| o.tile == tile))
            .map(|(k, v)| (*k, *v))
            .collect();
        let copies: Vec<NewRecord> = self
            .added
            .values()
            .filter(|a| a.tile == tile && !a.deleted)
            .map(|a| {
                let t = a.template.and_then(|id| self.objects.get(&id));
                a.record(t.map(|o| o.pos).unwrap_or(a.base), t.map(|o| o.heading).unwrap_or(a.base_heading))
            })
            .collect();
        let objects_dirty = !edits.is_empty() || !copies.is_empty();
        let ground_dirty = self.ground_is_dirty(tile);

        if !objects_dirty && !ground_dirty {
            return Ok(());
        }

        // The text as it was opened - never the file this wrote last time (see the module
        // docs: re-reading would apply an edit twice).
        let doc = self.loaded.get(&tile).ok_or(EditError::NoTile(tile.0, tile.1))?;
        let (source, text, encoding) = (doc.source.clone(), doc.text.clone(), doc.encoding);

        if objects_dirty {
            let (text, changed) = rewrite_tile(&text, &edits);
            let (text, added) = add_copies(&text, &copies);
            if changed + added == 0 && !edits.is_empty() {
                // the objects were indexed from this tile, so this is a bug worth saying aloud
                return Err(EditError::Other(format!(
                    "{} object(s) edited in tile ({}, {}) were not found in {}",
                    edits.len(),
                    tile.0,
                    tile.1,
                    source.display()
                )));
            }
            if changed + added > 0 {
                let out = self.target_of(&source);
                self.write(&out, &encode(&text, encoding), report)?;
                report.objects += changed + added;
                self.rebaseline(tile, text);
            }
        }

        if ground_dirty {
            let (bytes, hash) = {
                let t = self.terrain.get(&tile).ok_or(EditError::NoTile(tile.0, tile.1))?;
                (t.to_bytes(), terrain_hash(t))
            };
            let out = self.target_of(&companion(&source));
            self.write(&out, &bytes, report)?;
            // the ground on disk is this now, so the session goes clean for this tile
            self.terrain_hash.insert(tile, hash);
            report.grounded += 1;
        }
        Ok(())
    }

    /// Write one file, snapshotting it first when it is being overwritten in place.
    fn write(&mut self, out: &Path, data: &[u8], report: &mut SaveReport) -> Result<(), EditError> {
        if matches!(self.destination, Destination::InPlace) && out.exists() && !self.snapshotted.contains(out) {
            let backup = PathBuf::from(format!("{}{BACKUP_SUFFIX}", out.display()));
            if !backup.exists() {
                let previous = omsi_cfg::vfs::read(out).map_err(|e| EditError::io(out, e))?;
                std::fs::write(&backup, previous).map_err(|e| EditError::io(&backup, e))?;
                report.backups.push(backup);
            }
            self.snapshotted.insert(out.to_path_buf());
        }
        if let Some(dir) = out.parent() {
            std::fs::create_dir_all(dir).map_err(|e| EditError::io(dir, e))?;
        }
        std::fs::write(out, data).map_err(|e| EditError::io(out, e))?;
        log::debug!("editor: {}", out.display());
        report.files.push(out.to_path_buf());
        Ok(())
    }
}

/// The `.map.terrain` companion of a tile file (`tile_0_0.map` -> `tile_0_0.map.terrain`).
pub fn companion(src: &Path) -> PathBuf {
    match src.file_name() {
        Some(name) => src.with_file_name(format!("{}.terrain", name.to_string_lossy())),
        None => src.with_extension("terrain"),
    }
}
