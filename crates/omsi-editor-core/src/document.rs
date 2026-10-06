//! The open map: its tiles, its object index, what has been changed, and how it is written
//! back.
//!
//! A [`Document`] is the whole of an editing session's state and has no idea a window exists.
//! `omsi-editor` drives it - from a camera and panels in the window, from typed lines at the
//! terminal - and both get the same behaviour, which is the point of it living here.
//!
//! The game's own object editor is a third front end that is **not** on it, deliberately.
//! `omsi-app`'s `editor` keeps its own list of what was changed and its own `Added` record in
//! place of [`NewObject`], and shares this crate's writing end alone - [`rewrite_tile`],
//! [`add_copies`] and [`ObjectEdit`]. That editor is frozen: the standalone one is where map
//! editing goes, so no feature is added to the second copy again. Nothing here depends on it,
//! so the day it is deleted this Document stays as it is. See `docs/EDITOR.md`, stage 0.
//!
//! [`rewrite_tile`]: crate::rewrite_tile
//! [`add_copies`]: crate::add_copies
//! [`ObjectEdit`]: crate::ObjectEdit
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
#[derive(Clone, Debug, PartialEq)]
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
    /// How many tiles the map gained, files and ground and all.
    pub made: usize,
    /// Whether the map's own `global.cfg` was written - a tile added or taken away.
    pub listed: bool,
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
        let mut parts = vec![format!("{} object(s)", self.objects), format!("{} tile ground(s)", self.grounded)];
        if self.made > 0 {
            parts.push(format!("{} new tile(s)", self.made));
        }
        if self.listed {
            parts.push("the map's tile list".into());
        }
        format!(
            "Saved: {} in {} file(s){}",
            parts.join(", "),
            self.files.len(),
            if self.backups.is_empty() {
                String::new()
            } else {
                format!(" and {} snapshot(s)", self.backups.len())
            }
        )
    }
}

/// A `[map]` entry as the file has it: the tile's place, and the file it is written in.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TileEntry {
    pub x: i32,
    pub y: i32,
    pub file: String,
}

impl TileEntry {
    /// The entry for a tile at `x, y`, whose file is named the way OMSI names one.
    pub fn of(tile: TileId) -> TileEntry {
        TileEntry { x: tile.0, y: tile.1, file: tile_file_name(tile) }
    }
}

/// The file a tile is written in, as every OMSI map names it.
pub fn tile_file_name(tile: TileId) -> String {
    format!("tile_{}_{}.map", tile.0, tile.1)
}

/// What a tile held when it was taken out of the map: its file's text, the objects that file
/// listed and what this session had done to each, the copies placed in it, and its ground.
///
/// Kept so that putting the tile back puts all of it back - a removal is one step to take
/// back, so undoing one has to leave nothing behind. The file itself is never deleted, so
/// this is about what a *session* knew, not about what is on disk.
#[derive(Clone, Debug, PartialEq)]
pub struct HeldTile {
    /// The tile file's text as it was, and the encoding to write it back in.
    pub text: String,
    pub encoding: Encoding,
    /// The objects its file listed, and what this session had done to each.
    pub objects: Vec<(i64, ObjectRef, ObjectEdit)>,
    /// The copies placed in it this session.
    pub placed: Vec<NewObject>,
    /// Its ground as it stood, with the hash the last save left - `None` when nothing had
    /// loaded it, in which case a save reads it from the file as any other tile's.
    pub terrain: Option<(Terrain, u64)>,
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
    /// The map's own `global.cfg`: its text as it was read, and the encoding to put it back.
    /// Which tiles the map has *is* this file's `[map]` list, so adding a tile or taking one
    /// away is an edit to this text - made line by line and written back the way it came in,
    /// like every other edit here (see [`crate::tilemap`]).
    global_doc: TileDoc,
    /// The text a save compares the above against, so that "has the tile list changed?" is a
    /// comparison and not a flag some later undo would have to remember to clear - the same
    /// rule the ground's `terrain_hash` follows.
    global_baseline: String,
    /// The tiles this session added. Their files are not on disk yet, which nothing else in a
    /// [`Document`] is: every other change is a change to a file that already exists, so a
    /// save has to make these rather than rewrite them.
    new_tiles: HashSet<TileId>,
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
        // the map's folder, and the installation it sits in: where a tile's own file is looked
        // for, and what a copy's place under the content folder is measured from. Both are the
        // installation's, whatever file the reading below comes from.
        let map_dir = map_cfg.parent().map(Path::to_path_buf).unwrap_or_else(|| map_cfg.clone());
        let install_root = map_dir.parent().and_then(|p| p.parent()).map(|p| p.to_path_buf()).unwrap_or_else(|| map_dir.clone());
        // The map's own file, and which of it is read. Under `Destination::Content` a copy an
        // earlier save wrote is the one the game reads, so it is the one to read here too - the
        // same rule [`Document::tile_source`] follows for a tile, and what makes a tile added in
        // an earlier session still be there when the map is opened again.
        let source = match &destination {
            Destination::InPlace => map_cfg.clone(),
            Destination::Content(root) => {
                let copy = content_copy(&install_root, root, &map_cfg);
                if copy.exists() {
                    copy
                } else {
                    map_cfg.clone()
                }
            }
        };
        let global = GlobalCfg::load(&source).map_err(|e| EditError::Cfg { path: source.clone(), source: e })?;
        let tiles: HashMap<TileId, MapTileRef> = global.tiles.iter().map(|t| ((t.x, t.y), t.clone())).collect();
        let raw = omsi_cfg::vfs::read(&source).map_err(|e| EditError::io(&source, e))?;
        let (global_text, global_encoding) = decode(&raw);
        let mut doc = Document {
            map_cfg,
            map_dir,
            install_root,
            destination,
            global,
            global_doc: TileDoc { source, text: global_text.clone(), encoding: global_encoding },
            global_baseline: global_text,
            new_tiles: HashSet::new(),
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
        for id in self.tiles.keys().copied().collect::<Vec<_>>() {
            // the file a save would write this tile in, and not a lookup by name: a tile an
            // earlier session added is in the content folder and in no installation at all, and
            // a tile that has been saved once is the copy there (see [`Document::tile_source`])
            let Some(src) = self.tile_source(id) else { continue };
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

    /// The folder a record's own path is written against: the one holding `maps/`,
    /// `Sceneryobjects` and `Splines` - the installation, or the content folder the map was
    /// found under.
    ///
    /// A tile file names its objects as `Sceneryobjects\Berlin\House1.sco` and nothing else, so
    /// this is what turns that into a file on this machine
    /// ([`omsi_cfg::resolve_path`]) - and what [`crate::assets`] walks to list what may be
    /// placed.
    pub fn install_root(&self) -> &Path {
        &self.install_root
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
    ///
    /// Two questions and not one, because taking a tile out of the map changes no tile at all:
    /// whether any tile would be written, and whether the map's own list of them has changed.
    pub fn is_dirty(&self) -> bool {
        self.is_listed_dirty() || !self.dirty_tiles().is_empty()
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
        // a tile this session added has no file to rewrite, so a save has to make it - which
        // is why it is here even when nothing about it changed
        out.extend(self.new_tiles.iter().copied());
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
        content_copy(&self.install_root, root, src)
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

    // ---- the map's own list of tiles --------------------------------------------------

    /// The map's `global.cfg` as it stands, text and all.
    pub fn global_text(&self) -> &str {
        &self.global_doc.text
    }

    /// The map's own file as it now stands, parsed - the map list, the numbering an entry
    /// point counts, and the entry points themselves.
    ///
    /// A world keeps its own parse of that file, taken when the map was opened, and streams
    /// the tiles that parse names: a tile this session added was invisible to it, and one it
    /// took out went on being drawn, until the map was opened again. This is the same parse
    /// made again from the text a save would write, which is what a front end hands over
    /// (`View::adopt_tiles`).
    pub fn global_now(&self) -> GlobalCfg {
        GlobalCfg::parse(&omsi_cfg::CfgFile::from_str(&self.global_doc.source, &self.global_doc.text))
    }

    /// Whether the map's list of tiles has changed since the last save.
    ///
    /// Its own question and not a tile's: taking a tile *out* of a map changes no tile's
    /// contents at all, so "is anything unsaved?" is this **or** whether any tile would be
    /// written - see [`Document::is_dirty`].
    pub fn is_listed_dirty(&self) -> bool {
        self.global_doc.text != self.global_baseline
    }

    /// The tiles this session added, whose files are not on disk yet.
    pub fn new_tiles(&self) -> impl Iterator<Item = TileId> + '_ {
        self.new_tiles.iter().copied()
    }

    /// Where `tile` sits in the map's `[map]` list - the number an entry point and a track
    /// file name a tile by. `None` when the map does not list it.
    pub fn tile_index(&self, tile: TileId) -> Option<usize> {
        let entry = TileEntry::of(tile);
        crate::tilemap::map_entries(&self.global_doc.text)
            .iter()
            .position(|e| e.x == entry.x && e.y == entry.y && e.file.eq_ignore_ascii_case(&entry.file))
    }

    /// Whether the map has a timetable - any track file at all.
    ///
    /// It decides whether a tile can be taken out of the middle of the list. A track file
    /// names its tile by that tile's place in the list, and this project does not write
    /// `.ttr`, so a removal that renumbers anything is refused on a map that has one rather
    /// than left to point a route at the wrong tile without saying so.
    pub fn has_tracks(&self) -> bool {
        std::fs::read_dir(omsi_cfg::resolve_path(&self.map_dir, "TTData"))
            .map(|entries| entries.flatten().any(|e| e.path().extension().is_some_and(|x| x.eq_ignore_ascii_case("ttr"))))
            .unwrap_or(false)
    }

    /// Add the tile at `tile` to the map: a `[map]` entry for it, and an empty file and a flat
    /// ground for a save to write.
    ///
    /// The entry goes at the end of the list, so every number that names a tile goes on naming
    /// the same one. That is the whole reason adding is safe where taking away is not; see
    /// [`crate::tilemap`].
    pub fn add_tile(&mut self, tile: TileId) -> Result<(), EditError> {
        if self.tiles.contains_key(&tile) {
            return Err(EditError::Other(format!("tile ({}, {}) is already in this map", tile.0, tile.1)));
        }
        let entry = TileEntry::of(tile);
        if self.tiles.values().any(|t| t.file.eq_ignore_ascii_case(&entry.file)) {
            return Err(EditError::Other(format!("{} is already a tile of this map", entry.file)));
        }
        // The encoding the map's own tiles are in: a map written in Latin-1 should not gain one
        // tile of another kind. The first tile of the map's *list* decides - the list is an
        // order the map itself fixes, where the tiles' own map is a hash and would give a
        // different answer every run, so the same map could gain a UTF-8 tile one day and a
        // UTF-16 one the next.
        let encoding = {
            let mut listed: Vec<(usize, TileId)> = self.tiles.iter().map(|(id, t)| (t.index, *id)).collect();
            listed.sort_unstable();
            listed
                .iter()
                .find_map(|(_, id)| self.loaded.get(id))
                .map(|d| d.encoding)
                .unwrap_or(Encoding::Utf16Le)
        };
        let index = crate::tilemap::map_entries(&self.global_doc.text).len();
        self.global_doc.text = crate::tilemap::add_entry(&self.global_doc.text, entry.x, entry.y, &entry.file);
        let source = omsi_cfg::resolve_path(&self.map_dir, &entry.file);
        self.loaded.insert(tile, TileDoc { source, text: crate::newmap::EMPTY_TILE.to_string(), encoding });
        self.tiles.insert(tile, MapTileRef { x: tile.0, y: tile.1, file: entry.file, index });
        // flat ground, and the hash of it, so that shaping it later is what makes it dirty -
        // the file being missing is [`Document::new_tiles`]'s business, not the ground's
        let flat = Terrain::flat();
        self.terrain_hash.insert(tile, terrain_hash(&flat));
        self.terrain.insert(tile, flat);
        self.new_tiles.insert(tile);
        Ok(())
    }

    /// Take `tile` out of the map: its `[map]` entry, and everything it listed with it.
    ///
    /// What is refused, and why: [`crate::tilemap::can_remove`] for the list itself, and - for
    /// a removal that renumbers tiles - [`Document::has_tracks`], because a track file names a
    /// tile by number and this project does not write them.
    ///
    /// The tile's file is **not** deleted. It is no longer listed, and putting the tile back
    /// restores it exactly; a file somebody may have made by hand is not an edit to undo, and
    /// deleting one is not something a click should be able to do.
    pub fn remove_tile(&mut self, tile: TileId) -> Result<HeldTile, EditError> {
        let index = self.tile_index(tile).ok_or(EditError::NoTile(tile.0, tile.1))?;
        match crate::tilemap::can_remove(&self.global_doc.text, index) {
            crate::tilemap::Removal::NoSuchEntry => return Err(EditError::NoTile(tile.0, tile.1)),
            crate::tilemap::Removal::EntryPointStands => {
                return Err(EditError::Other("an entry point stands in that tile, so it cannot go".into()));
            }
            crate::tilemap::Removal::Renumbers if self.has_tracks() => {
                return Err(EditError::Other(
                    "this map has track files, which name tiles by their place in the list - take the tiles after it away first".into(),
                ));
            }
            _ => {}
        }
        let held = self.take_tile_contents(tile);
        self.global_doc.text = crate::tilemap::remove_entry(&self.global_doc.text, index);
        self.tiles.remove(&tile);
        self.new_tiles.remove(&tile);
        Ok(held)
    }

    /// Put a tile back where it was, with everything it held - the other half of
    /// [`Document::remove_tile`], and what its undo is.
    pub fn restore_tile(&mut self, tile: TileId, at: usize, held: HeldTile) -> Result<(), EditError> {
        if self.tiles.contains_key(&tile) {
            return Err(EditError::Other(format!("tile ({}, {}) is already in this map", tile.0, tile.1)));
        }
        let entry = TileEntry::of(tile);
        self.global_doc.text = crate::tilemap::insert_entry(&self.global_doc.text, at, entry.x, entry.y, &entry.file);
        let source = omsi_cfg::resolve_path(&self.map_dir, &entry.file);
        self.loaded.insert(tile, TileDoc { source, text: held.text, encoding: held.encoding });
        let index = self.tile_index(tile).unwrap_or(at);
        self.tiles.insert(tile, MapTileRef { x: tile.0, y: tile.1, file: entry.file, index });
        for (id, object, edit) in held.objects {
            self.objects.insert(id, object);
            if !edit.is_untouched() {
                self.edits.insert(id, edit);
            }
        }
        for copy in held.placed {
            self.added.insert(copy.id, copy);
        }
        if let Some((terrain, hash)) = held.terrain {
            self.terrain.insert(tile, terrain);
            self.terrain_hash.insert(tile, hash);
        }
        Ok(())
    }

    /// Everything `tile` holds, as a value: its file's text, the objects that file listed with
    /// what this session had done to each, the copies placed in it, and its ground.
    ///
    /// This is what a command captures before it adds a tile, so that taking the tile away
    /// again puts back exactly what was there.
    pub fn tile_contents(&self, tile: TileId) -> HeldTile {
        let (text, encoding) = self
            .loaded
            .get(&tile)
            .map(|d| (d.text.clone(), d.encoding))
            .unwrap_or((String::new(), Encoding::Utf16Le));
        let objects = self
            .objects
            .iter()
            .filter(|(_, o)| o.tile == tile)
            .map(|(id, o)| (*id, o.clone(), self.edit(*id)))
            .collect();
        let placed = self.added.values().filter(|a| a.tile == tile).cloned().collect();
        HeldTile {
            text,
            encoding,
            objects,
            placed,
            terrain: self.terrain.get(&tile).map(|t| (t.clone(), self.terrain_hash.get(&tile).copied().unwrap_or(0))),
        }
    }

    /// Everything `tile` holds, taken out of the index - see [`Document::tile_contents`].
    fn take_tile_contents(&mut self, tile: TileId) -> HeldTile {
        let held = self.tile_contents(tile);
        self.loaded.remove(&tile);
        for (id, _, _) in &held.objects {
            self.objects.remove(id);
            self.edits.remove(id);
        }
        for copy in &held.placed {
            self.added.remove(&copy.id);
        }
        self.terrain.remove(&tile);
        self.terrain_hash.remove(&tile);
        held
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
        // the map's own file, when a tile was added or taken away. It goes to the same place a
        // tile does, so a content-folder save writes the copy the game reads first.
        if self.is_listed_dirty() {
            let (source, text, encoding) = (self.global_doc.source.clone(), self.global_doc.text.clone(), self.global_doc.encoding);
            let out = self.target_of(&source);
            self.write(&out, &encode(&text, encoding), &mut report)?;
            self.global_baseline = text;
            report.listed = true;
        }
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
        // a tile the map has just gained: its file is not there to be rewritten, so a save
        // makes it - with whatever it holds, even when that is nothing
        let fresh = self.new_tiles.contains(&tile);

        if !fresh && !objects_dirty && !ground_dirty {
            return Ok(());
        }

        // The text as it was opened - never the file this wrote last time (see the module
        // docs: re-reading would apply an edit twice).
        let doc = self.loaded.get(&tile).ok_or(EditError::NoTile(tile.0, tile.1))?;
        let (source, text, encoding) = (doc.source.clone(), doc.text.clone(), doc.encoding);

        if fresh {
            let (text, added) = add_copies(&text, &copies);
            let out = self.target_of(&source);
            self.write(&out, &encode(&text, encoding), report)?;
            report.objects += added;
            report.made += 1;
            self.rebaseline(tile, text);
        } else if objects_dirty {
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

        // a new tile's ground goes with it, flat: the file does not exist to be left alone
        if ground_dirty || fresh {
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
        if fresh {
            self.new_tiles.remove(&tile);
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

/// Where a file read from `src` is written under `Destination::Content`: a copy that keeps the
/// map's place in the installation's layout, because that place is what the game looks a map
/// up by.
///
/// A file that is *already* under `root` is that copy - it is handed back as it is. Otherwise a
/// second save of a map whose own file is the copy (the launcher opens a map through the
/// content folder first) would write the tile beside the map folder instead of inside it.
fn content_copy(install_root: &Path, root: &Path, src: &Path) -> PathBuf {
    if src.starts_with(root) {
        return src.to_path_buf();
    }
    let rel = src.strip_prefix(install_root).unwrap_or_else(|_| src.file_name().map(Path::new).unwrap_or(Path::new("")));
    root.join(rel)
}
