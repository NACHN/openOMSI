//! An editing session against a small map on disk: open it, change it, write it, take the
//! change back.
//!
//! These are the tests that matter for the editor's promise. Two of them are the ones that
//! would have caught the bugs this design exists to avoid:
//!
//! * `a_move_changes_only_that_one_record` - everything else in the file, byte for byte;
//! * `saving_twice_does_not_apply_an_earlier_edit_twice` - the reason a save rewrites the text
//!   read at open rather than the file it wrote last time.

use glam::DVec3;
use omsi_editor_core::codec;
use omsi_editor_core::*;
use std::path::{Path, PathBuf};

/// A tile as OMSI's own editor writes it: UTF-8, CRLF, two objects.
const TILE_A: &str = "[version]\r\n11\r\n\r\n[object]\r\n0\r\nSceneryobjects\\Tiny\\a.sco\r\n100\r\n12\r\n34\r\n0.5\r\n90\r\n0\r\n0\r\n0\r\n\r\n[object]\r\n0\r\nSceneryobjects\\Tiny\\b.sco\r\n101\r\n5\r\n6\r\n0\r\n0\r\n0\r\n0\r\n0\r\n";

/// A second tile, saved as UTF-16LE with a byte order mark.
const TILE_B: &str = "[version]\r\n11\r\n\r\n[object]\r\n0\r\nSceneryobjects\\Tiny\\c.sco\r\n200\r\n1\r\n2\r\n0\r\n45\r\n0\r\n0\r\n0\r\n";

const GLOBAL: &str = "[map]\n0\n0\ntile_0_0.map\n\n[map]\n1\n0\ntile_1_0.map\n";

/// A throwaway OMSI 2 folder with one map in it.
struct Fixture {
    root: PathBuf,
}

impl Fixture {
    fn new(name: &str) -> Fixture {
        let root = std::env::temp_dir().join(format!("openomsi-editor-test-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let map = root.join("maps").join("Tiny");
        std::fs::create_dir_all(&map).unwrap();
        std::fs::write(map.join("global.cfg"), GLOBAL).unwrap();
        std::fs::write(map.join("tile_0_0.map"), TILE_A).unwrap();
        std::fs::write(map.join("tile_0_0.map.terrain"), omsi_map::Terrain::flat().to_bytes()).unwrap();
        std::fs::write(map.join("tile_1_0.map"), codec::encode(TILE_B, Encoding::Utf16Le)).unwrap();
        Fixture { root }
    }

    fn map_cfg(&self) -> PathBuf {
        self.root.join("maps").join("Tiny").join("global.cfg")
    }

    fn installed(&self, rel: &str) -> PathBuf {
        self.root.join(rel)
    }

    fn content(&self) -> PathBuf {
        self.root.join("content")
    }

    fn written(&self, rel: &str) -> PathBuf {
        self.content().join(rel)
    }

    fn text(&self, p: &Path) -> String {
        String::from_utf8(std::fs::read(p).unwrap()).unwrap()
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

fn open_copy(f: &Fixture) -> (Document, History) {
    (Document::open(f.map_cfg(), Destination::Content(f.content())).unwrap(), History::new())
}

fn move_by(doc: &mut Document, h: &mut History, id: i64, delta: DVec3) {
    h.apply(doc, Command::MoveObject { id, delta }).unwrap();
}

/// A copy of `template`, built the way a session builds one: the template's own record lines
/// travel with it, so the copy can outlive the template.
fn copy_of(doc: &Document, template: i64, file: &str, moved: DVec3, turned: f64) -> NewObject {
    let t = doc.object(template).expect("the template").clone();
    NewObject {
        template: Some(template),
        tile: t.tile,
        id: doc.next_id(),
        file: file.to_string(),
        base: t.pos,
        base_heading: t.heading,
        moved,
        turned,
        deleted: false,
        sco: format!("{}{file}", omsi_editor_core::document::folder_of(&t.file)),
        template_record: doc.object_record(template).expect("the template's record"),
    }
}

#[test]
fn opening_a_map_indexes_every_object_it_holds() {
    let f = Fixture::new("open");
    let doc = Document::open_in_place(f.map_cfg()).unwrap();
    assert_eq!(doc.tile_count(), 2);
    assert_eq!(doc.loaded_tiles(), 2);
    assert_eq!(doc.object_count(), 3);
    assert!(!doc.is_dirty());

    let a = doc.object(100).expect("object 100");
    assert_eq!(a.tile, (0, 0));
    assert_eq!(a.pos, DVec3::new(12.0, 34.0, 0.5));
    assert_eq!(a.heading, 90.0);
    assert_eq!(a.file_name(), "a.sco");
    assert_eq!(doc.object(200).expect("object 200").tile, (1, 0));
    assert_eq!(doc.object(101).unwrap().pos, DVec3::new(5.0, 6.0, 0.0));
    assert!(doc.object(999).is_none());
    // ids handed out later must clear everything the map already uses
    assert!(doc.next_id() > 200, "next id was {}", doc.next_id());
}

#[test]
fn a_move_changes_only_that_one_record() {
    let f = Fixture::new("move");
    let (mut doc, mut h) = open_copy(&f);
    move_by(&mut doc, &mut h, 100, DVec3::new(1.5, -1.0, 0.0));
    assert!(doc.is_dirty());

    let report = doc.save().unwrap();
    assert_eq!(report.objects, 1);
    assert_eq!(report.files.len(), 1);
    assert!(!doc.is_dirty(), "saving left the session dirty");

    let out = f.written("maps/Tiny/tile_0_0.map");
    let expected = TILE_A.replace("100\r\n12\r\n34", "100\r\n13.5\r\n33");
    assert_eq!(f.text(&out), expected, "more than the moved object changed");

    // the installation is never written to
    assert_eq!(f.text(&f.installed("maps/Tiny/tile_0_0.map")), TILE_A);
}

#[test]
fn saving_twice_does_not_apply_an_earlier_edit_twice() {
    let f = Fixture::new("twice");
    let (mut doc, mut h) = open_copy(&f);
    move_by(&mut doc, &mut h, 100, DVec3::new(1.0, 0.0, 0.0));
    doc.save().unwrap();
    // a second change in the same tile makes the whole tile be rewritten again
    move_by(&mut doc, &mut h, 101, DVec3::new(1.0, 0.0, 0.0));
    doc.save().unwrap();

    let text = f.text(&f.written("maps/Tiny/tile_0_0.map"));
    assert!(text.contains("100\r\n13\r\n34"), "object 100 moved twice: {text}");
    assert!(text.contains("101\r\n6\r\n6"), "object 101 did not move: {text}");
}

#[test]
fn undo_puts_the_session_back_to_clean() {
    let f = Fixture::new("undo");
    let (mut doc, mut h) = open_copy(&f);
    move_by(&mut doc, &mut h, 100, DVec3::new(1.0, 0.0, 0.0));
    assert!(doc.is_dirty());

    let label = h.undo(&mut doc).unwrap().expect("something to take back");
    assert_eq!(label, "Move object 100 (1, 0, 0 m)");
    assert!(!doc.is_dirty(), "undoing left the session dirty");
    // and the object is not even *listed* as edited any more (`Document::set_edit` drops an
    // edit that is untouched). A window that draws what has changed, and only what has
    // changed, therefore learns nothing from that list about what has just been put back -
    // it has to keep its own record of what it drew (`omsi-editor`'s `View::sync`).
    assert_eq!(doc.edits().count(), 0, "the undone edit is still listed");
    // and so a save writes nothing at all
    assert!(doc.save().unwrap().is_empty());
    assert!(!f.written("maps/Tiny/tile_0_0.map").exists());
}

#[test]
fn redo_writes_the_change_again() {
    let f = Fixture::new("redo");
    let (mut doc, mut h) = open_copy(&f);
    move_by(&mut doc, &mut h, 100, DVec3::new(1.0, 0.0, 0.0));
    h.undo(&mut doc).unwrap();
    assert!(!doc.is_dirty());

    h.redo(&mut doc).unwrap().expect("something to do again");
    assert!(doc.is_dirty());
    doc.save().unwrap();
    assert!(f.text(&f.written("maps/Tiny/tile_0_0.map")).contains("100\r\n13\r\n34"));
}

#[test]
fn a_new_change_drops_the_redo_stack() {
    let f = Fixture::new("drops");
    let (mut doc, mut h) = open_copy(&f);
    move_by(&mut doc, &mut h, 100, DVec3::new(1.0, 0.0, 0.0));
    h.undo(&mut doc).unwrap();
    assert!(h.can_redo());
    move_by(&mut doc, &mut h, 101, DVec3::new(1.0, 0.0, 0.0));
    assert!(!h.can_redo(), "redo survived a new change");
}

#[test]
fn a_taken_away_object_loses_its_record() {
    let f = Fixture::new("delete");
    let (mut doc, mut h) = open_copy(&f);
    h.apply(&mut doc, Command::SetEdit { id: 100, edit: ObjectEdit { deleted: true, ..Default::default() } }).unwrap();
    doc.save().unwrap();

    let text = f.text(&f.written("maps/Tiny/tile_0_0.map"));
    assert!(!text.contains("a.sco"), "the record is still there: {text}");
    assert!(text.contains("b.sco"), "the other object went too: {text}");
    // and taking it back brings the record home
    assert!(h.can_undo());
}

#[test]
fn a_utf16_tile_is_written_back_as_utf16() {
    let f = Fixture::new("utf16");
    let (mut doc, mut h) = open_copy(&f);
    move_by(&mut doc, &mut h, 200, DVec3::new(0.0, 1.0, 0.0));
    doc.save().unwrap();

    let bytes = std::fs::read(f.written("maps/Tiny/tile_1_0.map")).unwrap();
    assert_eq!(&bytes[..2], &[0xFF, 0xFE], "the byte order mark was dropped");
    let (text, enc) = codec::decode(&bytes);
    assert_eq!(enc, Encoding::Utf16Le);
    assert!(text.contains("200\r\n1\r\n3"), "the move is missing: {text:?}");
}

#[test]
fn a_placed_object_is_added_after_its_template() {
    let f = Fixture::new("place");
    let (mut doc, mut h) = open_copy(&f);
    let new = copy_of(&doc, 100, "lamp.sco", DVec3::new(2.0, 0.0, 0.0), 10.0);
    let id = new.id;
    h.apply(&mut doc, Command::PlaceObject(new)).unwrap();
    assert_eq!(doc.added_count(), 1);
    doc.save().unwrap();

    let text = f.text(&f.written("maps/Tiny/tile_0_0.map"));
    // the template's own place (12, 34, 0.5, heading 90) plus the copy's offset
    let want = format!("Sceneryobjects\\Tiny\\lamp.sco\r\n{id}\r\n14\r\n34\r\n0.5\r\n100");
    assert!(text.contains(&want), "the copy is not there: {text}");
    assert!(text.contains("Sceneryobjects\\Tiny\\a.sco"), "the template went: {text}");
    assert!(text.contains("Sceneryobjects\\Tiny\\b.sco"), "the other object went: {text}");
}

/// A copy is of the object *as the view shows it*, not of the record the file happens to hold.
///
/// The template here is turned to 180° and pushed 3 m north first. Copying used to read the
/// object's stored place and heading, so the copy landed where the template had been and
/// faced the way it used to - two metres off to the side of a direction nobody was looking in.
#[test]
fn a_copy_follows_the_template_as_it_stands_now() {
    let f = Fixture::new("copy-now");
    let mut s = Session::open(f.map_cfg(), Destination::Content(f.content())).unwrap();
    // object 100 sits at (12, 34, 0.5) facing 90°
    s.select(Some(100));
    s.turn_selected_to(180.0).unwrap();
    s.move_selected(DVec3::new(0.0, 3.0, 0.0)).unwrap();
    assert_eq!(s.selection().unwrap().position(), DVec3::new(12.0, 37.0, 0.5));
    assert_eq!(s.selection().unwrap().heading(), 180.0);

    s.place_copy(100, Some("lamp.sco".into()), None, 0.0).unwrap();
    let copy = s.selection().expect("the copy is selected");
    assert!(copy.placed, "a copy is not a map object yet");
    // two metres to the right of west (180°): (cos, -sin) = (-1, 0)
    assert_eq!(copy.heading(), 180.0, "the copy did not take the template's heading");
    assert_eq!(copy.position(), DVec3::new(10.0, 37.0, 0.5), "the copy is not beside the template");

    s.save().unwrap();
    let text = f.text(&f.written("maps/Tiny/tile_0_0.map"));
    // the record, measured against what the file had: (12, 34, 0.5) facing 90°
    let want = format!("Sceneryobjects\\Tiny\\lamp.sco\r\n{}\r\n10\r\n37\r\n0.5\r\n180", copy.id);
    assert!(text.contains(&want), "the copy's record is not what the view showed: {text}");
}

#[test]
fn a_placement_taken_back_leaves_nothing_to_save() {
    let f = Fixture::new("place-undo");
    let (mut doc, mut h) = open_copy(&f);
    let new = copy_of(&doc, 100, "lamp.sco", DVec3::ZERO, 0.0);
    h.apply(&mut doc, Command::PlaceObject(new)).unwrap();
    assert!(doc.is_dirty());

    h.undo(&mut doc).unwrap();
    assert_eq!(doc.added_count(), 0);
    assert!(!doc.is_dirty(), "taking the placement back left it dirty");
    assert!(doc.save().unwrap().is_empty());
}

#[test]
fn the_ground_brush_is_undone_by_putting_the_samples_back() {
    let f = Fixture::new("ground");
    let (mut doc, mut h) = open_copy(&f);
    // flat ground to start with
    assert_eq!(doc.ground_height(50.0, 50.0), Some(0.0));

    let cmd = Command::ShapeGround { at: DVec3::new(50.0, 50.0, 0.0), radius: 12.0, action: GroundAction::Raise(1.0) };
    h.apply(&mut doc, cmd).unwrap().expect("a stroke");
    assert!(doc.is_dirty());
    assert!((doc.ground_height(50.0, 50.0).unwrap() - 1.0).abs() < 1e-3);

    h.undo(&mut doc).unwrap();
    assert!(doc.ground_height(50.0, 50.0).unwrap().abs() < 1e-4);
    assert!(!doc.is_dirty(), "undoing the stroke left the ground dirty");
    assert!(doc.save().unwrap().is_empty());
}

#[test]
fn a_brush_stroke_writes_the_terrain_companion() {
    let f = Fixture::new("ground-save");
    let (mut doc, mut h) = open_copy(&f);
    let cmd = Command::ShapeGround { at: DVec3::new(50.0, 50.0, 0.0), radius: 12.0, action: GroundAction::Raise(2.0) };
    h.apply(&mut doc, cmd).unwrap();

    let report = doc.save().unwrap();
    assert_eq!(report.grounded, 1);
    assert!(report.objects == 0, "a ground stroke wrote object records");

    // the written companion reads back with the hill in it
    let bytes = std::fs::read(f.written("maps/Tiny/tile_0_0.map.terrain")).unwrap();
    let t = omsi_map::Terrain::parse(&bytes).unwrap();
    assert!((t.sample(50.0, 50.0) - 2.0).abs() < 1e-3);
    assert!(!doc.is_dirty());
}

#[test]
fn writing_in_place_snapshots_the_file_first() {
    let f = Fixture::new("in-place");
    let tile = f.installed("maps/Tiny/tile_0_0.map");
    let mut doc = Document::open_in_place(f.map_cfg()).unwrap();
    let mut h = History::new();
    move_by(&mut doc, &mut h, 100, DVec3::new(1.0, 0.0, 0.0));

    let report = doc.save().unwrap();
    assert_eq!(report.backups.len(), 1, "no snapshot was taken");
    assert!(f.text(&tile).contains("100\r\n13\r\n34"), "the map was not changed");
    assert_eq!(f.text(&PathBuf::from(format!("{}.openomsi-bak", tile.display()))), TILE_A, "the snapshot is not the original");

    // a second save must not overwrite the snapshot with the changed file
    move_by(&mut doc, &mut h, 100, DVec3::new(1.0, 0.0, 0.0));
    doc.save().unwrap();
    assert_eq!(f.text(&PathBuf::from(format!("{}.openomsi-bak", tile.display()))), TILE_A);
    assert!(f.text(&tile).contains("100\r\n14\r\n34"));
}

#[test]
fn a_save_is_a_checkpoint_the_history_does_not_cross() {
    let f = Fixture::new("checkpoint");
    let (mut doc, mut h) = open_copy(&f);
    move_by(&mut doc, &mut h, 100, DVec3::new(1.0, 0.0, 0.0));
    doc.save().unwrap();

    // the index now describes the map as it was written, not as it was opened
    assert_eq!(doc.object(100).expect("still there").pos, DVec3::new(13.0, 34.0, 0.5));
    assert!(!doc.is_dirty());
    assert_eq!(doc.revision(), 1);

    // the step from before the save is not offered again: it was measured against a baseline
    // that no longer exists
    assert_eq!(h.undo(&mut doc).unwrap(), None);
    assert!(!h.can_undo());

    // and a change made now is measured against the written map
    move_by(&mut doc, &mut h, 100, DVec3::new(1.0, 0.0, 0.0));
    doc.save().unwrap();
    assert!(f.text(&f.written("maps/Tiny/tile_0_0.map")).contains("100\r\n14\r\n34"));
}

#[test]
fn a_copy_moved_after_it_was_placed_is_saved_where_it_was_moved_to() {
    // A placed object has no record of its own yet, so an edit to it cannot live in the
    // same table the map's objects use - their records are in the tile's text and its is
    // not. Moving a copy and saving used to write it where it had first been put.
    let f = Fixture::new("place-move");
    let (mut doc, mut h) = open_copy(&f);
    let new = copy_of(&doc, 100, "lamp.sco", DVec3::new(4.0, 0.0, 0.0), 0.0);
    let id = new.id;
    h.apply(&mut doc, Command::PlaceObject(new)).unwrap();
    move_by(&mut doc, &mut h, id, DVec3::new(10.0, 0.0, 0.0));
    h.apply(&mut doc, Command::TurnObject { id, delta: 45.0 }).unwrap();
    doc.save().unwrap();

    let placed = doc.object(id).expect("the placed object is in the map");
    assert_eq!(placed.pos, DVec3::new(26.0, 34.0, 0.5), "placed at 16, 34 then moved 10 m east");
    assert_eq!(placed.heading, 135.0, "90 as the template has it, then turned 45");
    assert_eq!(doc.added_count(), 0, "it is an ordinary object now");
}

#[test]
fn a_placed_copy_taken_away_is_not_written() {
    let f = Fixture::new("place-delete");
    let (mut doc, mut h) = open_copy(&f);
    let new = copy_of(&doc, 100, "lamp.sco", DVec3::new(4.0, 0.0, 0.0), 0.0);
    let id = new.id;
    h.apply(&mut doc, Command::PlaceObject(new)).unwrap();
    h.apply(&mut doc, Command::SetEdit { id, edit: ObjectEdit { deleted: true, ..Default::default() } }).unwrap();
    doc.save().unwrap();
    assert!(doc.object(id).is_none(), "a copy taken away must not be written");
    assert!(doc.added_object(id).is_some(), "and it is still there to be put back");
}

#[test]
fn a_dragged_object_is_one_step_in_the_history() {
    // A gizmo dragged with the mouse asks for a place sixty times a second; the history
    // hears about the gesture once, and taking it back puts the object where the drag
    // started rather than one frame back.
    let f = Fixture::new("gesture");
    let mut s = Session::open(f.map_cfg(), Destination::Content(f.content())).unwrap();
    s.select(Some(100));
    let start = s.selection().unwrap().position();
    assert_eq!(s.begin_gesture(), Some(100));
    for i in 1..=40 {
        s.drag_to(ObjectEdit { moved: DVec3::new(i as f64 * 0.25, 3.0, 0.0), ..Default::default() });
    }
    s.end_gesture().unwrap();
    assert_eq!(s.history().depth(), 1, "one movement, one step");
    assert_eq!(s.selection().unwrap().position(), start + DVec3::new(10.0, 3.0, 0.0));
    s.undo().unwrap();
    assert_eq!(s.selection().unwrap().position(), start);
    s.redo().unwrap();
    assert_eq!(s.selection().unwrap().position(), start + DVec3::new(10.0, 3.0, 0.0));
    assert!(!s.history().can_undo() || s.history().depth() == 1);
}

#[test]
fn a_drag_that_goes_back_where_it_started_is_no_step() {
    let f = Fixture::new("gesture-noop");
    let mut s = Session::open(f.map_cfg(), Destination::Content(f.content())).unwrap();
    s.select(Some(100));
    s.begin_gesture();
    s.drag_to(ObjectEdit { moved: DVec3::new(5.0, 0.0, 0.0), ..Default::default() });
    s.drag_to(ObjectEdit::default());
    assert!(s.end_gesture().unwrap().is_none());
    assert_eq!(s.history().depth(), 0);
}

#[test]
fn a_drag_on_a_placed_copy_is_saved_where_it_was_dragged_to() {
    // the gizmo's own path, on the object kind whose edit lives somewhere else: a copy has
    // no record yet, so its `moved` is measured from the template rather than from a record
    let f = Fixture::new("gesture-placed");
    let mut s = Session::open(f.map_cfg(), Destination::Content(f.content())).unwrap();
    s.select(Some(100));
    s.place_copy(100, Some("lamp.sco".into()), Some(DVec3::new(20.0, 30.0, 0.5)), 0.0).unwrap();
    let id = s.selected_id().unwrap();
    let sel = s.selection().unwrap();
    assert!(sel.placed);
    assert_eq!(sel.position(), DVec3::new(20.0, 30.0, 0.5));
    assert_eq!(sel.object.pos, DVec3::new(12.0, 34.0, 0.5), "the template it was copied from");

    // dragged five metres east and turned 30 degrees, as a gizmo would ask for it: the edit
    // is absolute against the reference the copy was made from, not a delta from the last frame
    assert_eq!(s.begin_gesture(), Some(id));
    s.drag_to(ObjectEdit { moved: DVec3::new(13.0, -4.0, 0.0), turned: 30.0, deleted: false });
    s.end_gesture().unwrap();
    s.save().unwrap();

    let placed = s.doc().object(id).expect("written into the map");
    assert_eq!(placed.pos, DVec3::new(25.0, 30.0, 0.5));
    assert_eq!(placed.heading, 120.0, "90 as the template has it, then 30");
    assert!(s.undo().unwrap().is_none(), "a save is a checkpoint the history does not cross");
    assert_eq!(s.doc().object(id).expect("still in the map").pos, DVec3::new(25.0, 30.0, 0.5));
}

#[test]
fn a_record_s_place_is_where_it_stands_on_the_map_not_where_its_tile_puts_it() {
    // A tile file's object coordinates start at the tile's own corner: object 200 is 1 m
    // across and 2 m up tile (1, 0), which is 301 m across the map. Everything above the
    // index - the crosshair, an entry point, a copy measured from its template - works in the
    // map's own frame, and the two spaces look enough alike to be mixed up.
    let f = Fixture::new("world-pos");
    let doc = Document::open_in_place(f.map_cfg()).unwrap();
    assert_eq!(doc.object(200).unwrap().pos, DVec3::new(301.0, 2.0, 0.0));
    assert_eq!(doc.object(100).unwrap().pos, DVec3::new(12.0, 34.0, 0.5), "tile (0, 0) is its own corner");
}

#[test]
fn a_move_to_a_place_on_the_map_measures_it_from_where_the_record_stands() {
    let f = Fixture::new("move-to");
    let mut s = Session::open(f.map_cfg(), Destination::Content(f.content())).unwrap();
    s.select(Some(200));
    // nine metres across and ten up the map, from where it stands
    s.move_selected_to(DVec3::new(310.0, 12.0, 0.0)).unwrap();
    s.save().unwrap();
    let text = codec::decode(&std::fs::read(f.written("maps/Tiny/tile_1_0.map")).unwrap()).0;
    // the record keeps the tile's own coordinates: 1 + 9 across, 2 + 10 up
    assert!(text.contains("10\r\n12"), "expected 10, 12 in the record, got:\n{text}");
}

#[test]
fn dropping_an_object_on_the_ground_leaves_its_height_over_it_alone() {
    // The crosshair's place carries the height of the ground there, and a record's z is a
    // height *over* the ground - the game adds the terrain back under the object. Adding the
    // two together dropped the object as many metres above the ground as the map is high.
    let f = Fixture::new("drop-on-ground");
    let mut s = Session::open(f.map_cfg(), Destination::Content(f.content())).unwrap();
    s.select(Some(100));
    // (object 100 stands at z 0.5 over the ground, as the record has it)
    let at = DVec3::new(40.0, 50.0, 9.0);
    s.move_selected_on_the_ground(at).unwrap();
    let sel = s.selection().unwrap();
    assert_eq!(sel.position(), DVec3::new(40.0, 50.0, 0.5), "across and along to the place, and the height as it was");
}

#[test]
fn a_copy_of_an_object_in_another_tile_is_written_inside_that_tile() {
    let f = Fixture::new("copy-other-tile");
    let mut s = Session::open(f.map_cfg(), Destination::Content(f.content())).unwrap();
    s.select(Some(200));
    // two metres east of it on the map, which is inside the same tile
    s.place_copy(200, Some("lamp.sco".into()), Some(DVec3::new(303.0, 2.0, 0.0)), 0.0).unwrap();
    let id = s.selected_id().unwrap();
    s.save().unwrap();
    let text = codec::decode(&std::fs::read(f.written("maps/Tiny/tile_1_0.map")).unwrap()).0;
    let copy = object_records(&text).into_iter().find(|r| r.id == id).expect("the copy's own record");
    // the record is measured from the tile's corner: 303 - 300 across, 2 up
    assert_eq!(copy.pos, DVec3::new(3.0, 2.0, 0.0), "a copy written in the map's own coordinates lands a tile away");
    assert!(copy.file.contains("lamp.sco"));
}

#[test]
fn an_object_placed_and_saved_becomes_an_ordinary_object() {
    let f = Fixture::new("place-save");
    let (mut doc, mut h) = open_copy(&f);
    let new = copy_of(&doc, 100, "lamp.sco", DVec3::new(4.0, 0.0, 0.0), 0.0);
    let id = new.id;
    h.apply(&mut doc, Command::PlaceObject(new)).unwrap();
    doc.save().unwrap();

    // it is no longer a "placed this session" object: it is in the map
    assert_eq!(doc.added_count(), 0);
    let placed = doc.object(id).expect("the placed object is in the map");
    assert_eq!(placed.pos, DVec3::new(16.0, 34.0, 0.5));
    assert!(placed.file.contains("lamp.sco"));
    assert!(!doc.is_dirty());
    // and the id will not be handed out twice
    assert!(doc.next_id() > id);
}

#[test]
fn a_copy_whose_template_is_taken_away_in_the_same_save_survives() {
    // Found by running the editor: copying an object and taking that same object away, then
    // saving, used to lose the copy without a word - the record it was written after had
    // already been removed. The copy now carries the template's record with it, and lands at
    // the end of the file when there is nothing left to follow.
    let f = Fixture::new("copy-orphan");
    let (mut doc, mut h) = open_copy(&f);
    let new = copy_of(&doc, 100, "lamp.sco", DVec3::new(3.0, 0.0, 0.0), 0.0);
    let id = new.id;
    h.apply(&mut doc, Command::PlaceObject(new)).unwrap();
    h.apply(&mut doc, Command::SetEdit { id: 100, edit: ObjectEdit { deleted: true, ..Default::default() } }).unwrap();
    let report = doc.save().unwrap();

    assert_eq!(report.objects, 2, "the change and the copy");
    let text = f.text(&f.written("maps/Tiny/tile_0_0.map"));
    assert!(!text.contains("a.sco\r\n100"), "the template is still there: {text}");
    assert!(text.contains(&format!("lamp.sco\r\n{id}\r\n15\r\n34\r\n0.5\r\n90")), "the copy was lost: {text}");
    assert!(text.contains("b.sco\r\n101"), "the unrelated object went: {text}");
}

#[test]
fn a_batch_is_taken_back_as_one_step() {
    let f = Fixture::new("batch");
    let (mut doc, mut h) = open_copy(&f);
    let cmd = Command::Batch(vec![
        Command::MoveObject { id: 100, delta: DVec3::new(1.0, 0.0, 0.0) },
        Command::TurnObject { id: 101, delta: 15.0 },
    ]);
    h.apply(&mut doc, cmd).unwrap().expect("a gesture");
    assert_eq!(h.depth(), 1, "a batch must be one step, not two");
    doc.save().unwrap();

    let text = f.text(&f.written("maps/Tiny/tile_0_0.map"));
    assert!(text.contains("100\r\n13\r\n34"), "{text}");
    assert!(text.contains("101\r\n5\r\n6\r\n0\r\n15"), "{text}");

    // one undo takes both back
    h.undo(&mut doc).unwrap();
    assert!(!doc.is_dirty());
}

// ---- the map's own list of tiles -------------------------------------------------------

#[test]
fn an_added_tile_is_written_and_a_map_opened_again_has_it() {
    let f = Fixture::new("add-tile");
    let mut session = Session::open(f.map_cfg(), Destination::Content(f.content())).unwrap();

    let tile = (2, 0);
    assert!(!session.has_tile(tile));
    session.add_tile(tile).unwrap();
    assert!(session.has_tile(tile));
    assert!(session.doc().is_dirty(), "a map that gained a tile is not clean");
    // it is the third entry of the list, so nothing that numbers a tile has moved
    assert_eq!(session.tile_index(tile), Some(2));

    let report = session.save().unwrap();
    assert!(report.listed, "the map's own file was not written");
    assert_eq!(report.made, 1);
    assert!(!session.doc().is_dirty());

    // the copy the game reads first holds the tile: the list, the file, and its ground
    let copy = f.content().join("maps").join("Tiny");
    let global = std::fs::read_to_string(copy.join("global.cfg")).unwrap();
    assert!(global.contains("tile_2_0.map"), "{global:?}");
    assert!(copy.join("tile_2_0.map").exists());
    assert!(copy.join("tile_2_0.map.terrain").exists());
    // and the map that was there is untouched, byte for byte
    assert_eq!(f.text(&f.map_cfg()), GLOBAL);

    // a session opened on it again finds the tile, in its place in the list
    let again = Session::open(f.map_cfg(), Destination::Content(f.content())).unwrap();
    assert!(again.has_tile(tile));
    assert_eq!(again.tiles(), vec![(0, 0), (1, 0), (2, 0)]);
    let t = omsi_map::Tile::load(&copy.join("tile_2_0.map")).unwrap();
    assert!(t.objects.is_empty() && t.splines.is_empty());
}

#[test]
fn a_tile_taken_out_goes_with_what_was_in_it_and_comes_back_whole() {
    let f = Fixture::new("remove-tile");
    let mut session = Session::open(f.map_cfg(), Destination::Content(f.content())).unwrap();
    // the second tile of the fixture holds one object
    assert_eq!(session.objects_in((1, 0)), 1);
    session.select(Some(200));
    assert!(session.selection().is_some());

    session.remove_tile((1, 0)).unwrap();
    assert!(!session.has_tile((1, 0)));
    assert_eq!(session.objects_in((1, 0)), 0);
    // what was chosen stood in that tile, and something no longer in the map cannot go on
    // being chosen
    assert_eq!(session.selected_id(), None);
    assert!(session.doc().is_dirty());

    // one step back: the tile, the object that was in it, and the list as it was
    session.undo().unwrap();
    assert!(session.has_tile((1, 0)));
    assert_eq!(session.objects_in((1, 0)), 1);
    assert_eq!(session.tiles(), vec![(0, 0), (1, 0)]);

    // and a save of the removal leaves the tile's own file where it is
    session.remove_tile((1, 0)).unwrap();
    session.save().unwrap();
    let global = std::fs::read_to_string(f.content().join("maps").join("Tiny").join("global.cfg")).unwrap();
    assert!(!global.contains("tile_1_0.map"), "{global:?}");
    assert!(f.installed("maps/Tiny/tile_1_0.map").exists(), "the tile's own file was deleted");
}

#[test]
fn a_map_with_tracks_refuses_a_removal_that_would_renumber_the_tiles() {
    let f = Fixture::new("tracks");
    // a timetable of any kind: a track file names its tile by the tile's place in the list
    std::fs::create_dir_all(f.installed("maps/Tiny/TTData")).unwrap();
    std::fs::write(f.installed("maps/Tiny/TTData/route.ttr"), "[track_entry]\r\n").unwrap();

    let mut session = Session::open(f.map_cfg(), Destination::Content(f.content())).unwrap();
    // (0, 0) is the first of two, so taking it out would move the other down to number 0
    let err = session.remove_tile((0, 0)).unwrap_err();
    assert!(format!("{err}").contains("track"), "{err}");
    assert!(session.has_tile((0, 0)), "the tile went anyway");

    // the last one can still go: nothing after it changes its number
    session.remove_tile((1, 0)).unwrap();
    assert!(!session.has_tile((1, 0)));
}

#[test]
fn an_entry_point_standing_in_a_tile_stops_that_tile_from_going() {
    let f = Fixture::new("entry-point");
    let global = format!("[entrypoints]\n1\n0\n0\n0\n150\n0\n150\n0\n0\n0\n1\n0\nStart\n\n{GLOBAL}");
    std::fs::write(f.map_cfg(), &global).unwrap();

    let mut session = Session::open(f.map_cfg(), Destination::Content(f.content())).unwrap();
    let err = session.remove_tile((0, 0)).unwrap_err();
    assert!(format!("{err}").contains("entry point"), "{err}");
    assert!(session.has_tile((0, 0)), "the tile went anyway");
}

#[test]
fn the_tile_list_is_edited_as_text_and_the_rest_of_the_file_is_kept() {
    let f = Fixture::new("global-text");
    // a map whose own file holds a keyword this project does not know
    let text = format!("[friendlyname]\nTiny\n\n[myownkeyword]\nwhatever it means\n\n{GLOBAL}");
    std::fs::write(f.map_cfg(), &text).unwrap();

    let mut session = Session::open(f.map_cfg(), Destination::Content(f.content())).unwrap();
    session.add_tile((2, 0)).unwrap();
    session.save().unwrap();

    let saved = std::fs::read_to_string(f.content().join("maps").join("Tiny").join("global.cfg")).unwrap();
    let inserted = "\n[map]\n2\n0\ntile_2_0.map\n";
    assert!(saved.contains(inserted), "{saved:?}");
    assert_eq!(saved.replace(inserted, ""), text, "something else in the file changed");
}

// ---- an object out of the content folder -----------------------------------------------

/// A `.sco` in the content folder that no map holds - what the assets list offers. Returns it
/// spelled the way a record spells it.
fn put_a_sco(f: &Fixture, rel: &str) -> String {
    let p = f.installed(rel);
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(&p, "[mesh]\r\nthing.o3d\r\n").unwrap();
    rel.replace('/', "\\")
}

#[test]
fn an_object_no_map_holds_can_be_placed_and_is_there_after_a_reopen() {
    let f = Fixture::new("place-fresh");
    let sco = put_a_sco(&f, "Sceneryobjects/New/thing.sco");
    let mut session = Session::open(f.map_cfg(), Destination::Content(f.content())).unwrap();
    let in_the_map = session.doc().object_count();

    session.place_asset(&sco, DVec3::new(20.0, 30.0, 0.0), 45.0).unwrap();
    let id = session.selected_id().expect("the new object is chosen");
    assert_eq!(session.doc().object_count(), in_the_map, "a placed object is not a map object yet");
    assert_eq!(session.doc().added_count(), 1, "nothing new was placed");
    assert!(session.doc().is_dirty(), "a map that grew is not clean");

    let report = session.save().unwrap();
    assert_eq!(report.objects, 1, "{}", report.describe());

    // the record is in the file the game reads, and it says where the object stands - the
    // place is tile-local, which for tile (0, 0) is the same numbers
    let written = f.text(&f.written("maps/Tiny/tile_0_0.map"));
    let added = format!("\r\n[object]\r\n0\r\n{sco}\r\n{id}\r\n20\r\n30\r\n0\r\n45\r\n0\r\n0\r\n0\r\n\r\n");
    assert!(written.contains(&added), "{written:?}");
    // and nothing else in that file changed: not a line, not an ending
    assert_eq!(written.replace(&added, ""), TILE_A, "something else in the tile changed");
    // the map that was there is untouched, and the record is only in the copy
    assert_eq!(f.text(&f.map_cfg()).lines().count(), GLOBAL.lines().count());
    assert!(!f.text(&f.installed("maps/Tiny/tile_0_0.map")).contains(&sco));

    // opened again, the object is in the map like any other - and nothing is unsaved
    let again = Session::open(f.map_cfg(), Destination::Content(f.content())).unwrap();
    assert!(!again.doc().is_dirty(), "a map written and opened again has nothing to save");
    let back = again.doc().objects().find(|(_, o)| o.file == sco).expect("the placed object").1.clone();
    assert_eq!(back.tile, (0, 0));
    assert_eq!(back.pos, DVec3::new(20.0, 30.0, 0.0));
    assert_eq!(back.heading, 45.0);
    assert_eq!(again.objects_in((0, 0)), 3);
    assert_eq!(again.selected_id(), None);
}

#[test]
fn a_place_that_cannot_be_a_record_is_refused_and_changes_nothing() {
    let f = Fixture::new("place-refusals");
    let sco = put_a_sco(&f, "Sceneryobjects/New/thing.sco");
    let mut session = Session::open(f.map_cfg(), Destination::Content(f.content())).unwrap();

    // a file that is not a scenery object at all
    let e = session.place_asset("Sceneryobjects\\New\\thing.o3d", DVec3::new(1.0, 1.0, 0.0), 0.0).unwrap_err();
    assert!(format!("{e}").contains(".sco"), "{e}");
    // a `.sco` the content folder does not have
    let e = session.place_asset("Sceneryobjects\\Nowhere\\thing.sco", DVec3::new(1.0, 1.0, 0.0), 0.0).unwrap_err();
    assert!(format!("{e}").contains("no such file"), "{e}");
    // a place in a tile the map does not list: a record there would be dropped by the first
    // save, so it is refused rather than lost in silence
    let far = omsi_map::tile_size() * 9.0 + 1.0;
    let e = session.place_asset(&sco, DVec3::new(far, 1.0, 0.0), 0.0).unwrap_err();
    assert!(format!("{e}").contains("not in this map"), "{e}");

    // and a refusal is not an edit: nothing to take back, nothing to write
    assert_eq!(session.doc().added_count(), 0);
    assert!(!session.doc().is_dirty());
    assert!(!session.history().can_undo());
    assert_eq!(session.selected_id(), None);
}

#[test]
fn putting_an_object_down_is_one_step_to_take_back() {
    let f = Fixture::new("place-undo");
    let sco = put_a_sco(&f, "Sceneryobjects/New/thing.sco");
    let mut session = Session::open(f.map_cfg(), Destination::Content(f.content())).unwrap();
    session.place_asset(&sco, DVec3::new(20.0, 30.0, 0.0), 0.0).unwrap();
    let id = session.selected_id().unwrap();

    session.undo().unwrap();
    assert_eq!(session.doc().added_count(), 0);
    assert!(!session.doc().is_dirty(), "an undone placement is not something to write");

    session.redo().unwrap();
    assert_eq!(session.doc().added_count(), 1);
    assert_eq!(session.doc().added_object(id).map(|a| a.sco.clone()), Some(sco));
}

#[test]
fn arming_a_file_is_not_an_edit_and_a_blank_name_arms_nothing() {
    let f = Fixture::new("arm");
    let sco = put_a_sco(&f, "Sceneryobjects/New/thing.sco");
    let mut session = Session::open(f.map_cfg(), Destination::Content(f.content())).unwrap();
    assert_eq!(session.armed_asset(), None);

    session.arm_asset(Some(sco.clone()));
    assert_eq!(session.armed_asset(), Some(sco.as_str()));
    assert!(!session.doc().is_dirty(), "arming is not a change to the map");
    assert!(!session.history().can_undo());

    // a name that is nothing is not a file, and the tool must not be left holding one
    session.arm_asset(Some("   ".into()));
    assert_eq!(session.armed_asset(), None);
}

#[test]
fn a_brand_new_tile_takes_a_brand_new_object() {
    let f = Fixture::new("place-in-new-tile");
    let sco = put_a_sco(&f, "Sceneryobjects/New/thing.sco");
    let mut session = Session::open(f.map_cfg(), Destination::Content(f.content())).unwrap();
    let ts = omsi_map::tile_size();
    session.add_tile((2, 0)).unwrap();
    session.place_asset(&sco, DVec3::new(ts * 2.0 + 7.0, 9.0, 0.0), 90.0).unwrap();

    let report = session.save().unwrap();
    assert!(report.listed, "the map's own file was not written");
    assert_eq!(report.made, 1);

    // the tile the map just gained, with the object on it - and the place is what it is
    // *within the tile*, not its place on the map
    let written = f.text(&f.written("maps/Tiny/tile_2_0.map"));
    assert!(written.contains(&format!("{sco}\r\n")), "{written:?}");
    assert!(written.contains("\r\n7\r\n9\r\n0\r\n90\r\n"), "the place is not tile-local: {written:?}");
    // one blank line between the tile's own keywords and the record, not two
    assert!(written.contains("[variable_terrain]\r\n\r\n[object]\r\n"), "{written:?}");

    let again = Session::open(f.map_cfg(), Destination::Content(f.content())).unwrap();
    assert!(again.has_tile((2, 0)));
    assert_eq!(again.objects_in((2, 0)), 1);
}
