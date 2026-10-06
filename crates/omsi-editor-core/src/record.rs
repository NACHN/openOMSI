//! One object record inside a tile file: moved, turned, taken away or copied.
//!
//! A tile file is a list of records, each one a keyword line (`[object]`, `[attachObj]`,
//! `[spline]`, …) followed by a fixed run of lines. This module rewrites an `[object]` record
//! and nothing else: every other line, every other keyword, every line ending and the file's
//! encoding are passed through byte for byte. That is what makes editing a modded map safe.
//!
//! `[attachObj]` records are deliberately not covered - an attached object hangs on a parent
//! and moving it on its own would break the attachment. The same limit the in-game editor
//! had, kept on purpose.

use crate::codec::{body, ending, num};
use glam::DVec3;
use hashbrown::HashMap;

/// What has been done to one object: moved by `moved` (m), turned by `turned` (degrees
/// clockwise, as a map object's heading), or taken away.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct ObjectEdit {
    pub moved: DVec3,
    pub turned: f64,
    pub deleted: bool,
}

impl ObjectEdit {
    /// Nothing done to it - its record as the map file has it.
    pub fn is_untouched(&self) -> bool {
        self.moved == DVec3::ZERO && self.turned == 0.0 && !self.deleted
    }
}

/// A new object for the tile file: a copy of `template`'s record with its own id, another
/// file name of the same folder (or the same), moved and turned from the template.
///
/// The template's record lines travel with the copy rather than being looked up when the file
/// is written. That is what lets a copy outlive its template: taking an object away and
/// placing a copy of it in the same save used to lose the copy silently, because the record it
/// was written after had already gone. A copy is also written at the end of the file when its
/// template's record is not there.
#[derive(Clone, Debug, PartialEq)]
pub struct NewRecord {
    pub template: i64,
    pub id: i64,
    pub file: String,
    pub moved: DVec3,
    pub turned: f64,
    /// The template's own record, its line endings included.
    pub template_lines: Vec<String>,
}

/// How many lines [`blank_object_record`] writes - a record with no labels.
const RECORD_LINES: usize = 11;

/// The lines a brand-new `[object]` record is made of: an object placed from a file in the
/// content folder, which no map holds yet and which therefore has no template to be copied
/// from.
///
/// The shape is the one [`object_records`] reads back and [`rewrite_record`] writes in -
/// keyword, detail level, file, id, x, y, z, heading, and then the two angles and the label
/// count a version 12 and later tile carries. Every number is already final: a record made
/// from these lines is placed exactly where they say, which is why the deltas of a copy
/// ([`NewRecord::moved`], [`NewRecord::turned`]) are left at zero for one.
///
/// `at` is *tile-local* and z is a height **over the ground**, because that is the space a
/// tile file writes an object in - pass `0` for z and the object stands on the terrain, which
/// is where a newly placed object goes.
///
/// Empty labels (a count of `0`) and not a name: what an object is called is the map maker's
/// to say, and a name invented here would be text nobody typed.
pub fn blank_object_record(sco: &str, id: i64, at: DVec3, heading: f64) -> Vec<String> {
    let mut out: Vec<String> = Vec::with_capacity(RECORD_LINES);
    out.push("[object]\r\n".to_string());
    // the detail level a version 9 and later tile writes
    out.push("0\r\n".to_string());
    // (the file as a record writes it, whatever separators it arrived with)
    out.push(format!("{}\r\n", sco.replace('/', "\\")));
    out.push(format!("{id}\r\n"));
    out.push(format!("{}\r\n", num(at.x)));
    out.push(format!("{}\r\n", num(at.y)));
    out.push(format!("{}\r\n", num(at.z)));
    out.push(format!("{}\r\n", num(heading)));
    // pitch, bank, and the count of the object's labels
    out.push("0\r\n".to_string());
    out.push("0\r\n".to_string());
    out.push("0\r\n".to_string());
    out
}

/// The tile file with `copies` added, each record after its template's - or at the end for a
/// template that is not in the file. Returns the text and how many were added.
pub fn add_copies(text: &str, copies: &[NewRecord]) -> (String, usize) {
    let copies: Vec<&NewRecord> = copies.iter().filter(|c| !c.template_lines.is_empty()).collect();
    if copies.is_empty() {
        return (text.to_string(), 0);
    }
    let lines: Vec<&str> = text.split_inclusive('\n').collect();
    let mut out = String::with_capacity(text.len() + copies.len() * 200);
    let mut placed: Vec<i64> = Vec::new();
    let mut added = 0;
    let mut i = 0;
    while i < lines.len() {
        let is_object = body(lines[i]).trim().eq_ignore_ascii_case("[object]");
        let id = lines.get(i + 3).and_then(|l| body(l).trim().parse::<i64>().ok());
        let mine: Vec<&&NewRecord> = if is_object { copies.iter().filter(|c| Some(c.template) == id).collect() } else { Vec::new() };
        if mine.is_empty() {
            out.push_str(lines[i]);
            i += 1;
            continue;
        }
        // the template's record, up to the next keyword
        let start = i;
        i += 1;
        while i < lines.len() && !body(lines[i]).trim_start().starts_with('[') {
            i += 1;
        }
        let record = &lines[start..i];
        for l in record {
            out.push_str(l);
        }
        // (a blank line between records, as the editor writes them)
        let eol = ending(record[0]).to_string();
        if !record.last().map(|l| body(l).trim().is_empty()).unwrap_or(false) {
            out.push_str(&eol);
        }
        for c in mine {
            out.push_str(&rewrite_record(c));
            if !c.template_lines.last().map(|l| body(l).trim().is_empty()).unwrap_or(false) {
                out.push_str(&eol);
            }
            placed.push(c.id);
            added += 1;
        }
    }
    // a copy whose template this save takes away has no record to follow: it goes at the end,
    // rather than being dropped without a word. An object placed from a file rather than
    // copied is always one of these - it has no template at all, so the end is its place.
    for c in copies.iter().filter(|c| !placed.contains(&c.id)) {
        let eol = c.template_lines.first().map(|l| ending(l)).unwrap_or("\n").to_string();
        if !out.is_empty() && !out.ends_with('\n') {
            out.push('\n');
        }
        // (a blank line between records, as the editor writes them - the end of the file is
        // no different from the middle of it, and a record written straight after another is
        // the one thing a person reading the file would call wrong)
        if !out.is_empty() && !ends_with_blank_line(&out) {
            out.push_str(&eol);
        }
        out.push_str(&rewrite_record(c));
        if !c.template_lines.last().map(|l| body(l).trim().is_empty()).unwrap_or(false) {
            out.push_str(&eol);
        }
        added += 1;
    }
    (out, added)
}

/// Whether `text` ends with a blank line - what OMSI's own editor leaves between two records.
fn ends_with_blank_line(text: &str) -> bool {
    match text.split_inclusive('\n').next_back() {
        Some(last) => body(last).trim().is_empty(),
        None => true,
    }
}

/// The copy's record: the template's own lines with its file, its id and its place changed.
fn rewrite_record(c: &NewRecord) -> String {
    let mut out = String::with_capacity(256);
    for (k, l) in c.template_lines.iter().enumerate() {
        let b = body(l);
        match k {
            // the file: the copy's name in the template's folder
            2 => match b.rfind(['\\', '/']) {
                Some(p) => out.push_str(&format!("{}{}", &b[..=p], c.file)),
                None => out.push_str(&c.file),
            },
            3 => out.push_str(&c.id.to_string()),
            4..=7 => match b.trim().parse::<f64>() {
                Ok(v) => out.push_str(&num(v + [c.moved.x, c.moved.y, c.moved.z, c.turned][k - 4])),
                Err(_) => out.push_str(b),
            },
            _ => out.push_str(b),
        }
        out.push_str(ending(l));
    }
    out
}

/// The tile file with the edits applied to its `[object]` records (by map id): a moved or
/// turned object gets its position (x, y, height over the ground) and heading changed, a
/// deleted one loses its record. Everything else stays as it was, line endings included.
/// Returns the text and how many records changed.
pub fn rewrite_tile(text: &str, edits: &HashMap<i64, ObjectEdit>) -> (String, usize) {
    let lines: Vec<&str> = text.split_inclusive('\n').collect();
    let mut out = String::with_capacity(text.len());
    let mut changed = 0;
    let mut i = 0;
    while i < lines.len() {
        let is_object = body(lines[i]).trim().eq_ignore_ascii_case("[object]");
        let id = lines.get(i + 3).and_then(|l| body(l).trim().parse::<i64>().ok());
        let edit = if is_object { id.and_then(|id| edits.get(&id)) } else { None };
        let Some(e) = edit else {
            out.push_str(lines[i]);
            i += 1;
            continue;
        };
        changed += 1;
        if e.deleted {
            // the record up to the next keyword
            i += 1;
            while i < lines.len() && !body(lines[i]).trim_start().starts_with('[') {
                i += 1;
            }
            continue;
        }
        // [object], 0, file, id, x, y, z, heading, …
        for k in 0..4 {
            out.push_str(lines[i + k]);
        }
        let deltas = [e.moved.x, e.moved.y, e.moved.z, e.turned];
        for (k, d) in deltas.iter().enumerate() {
            let Some(l) = lines.get(i + 4 + k) else { break };
            match body(l).trim().parse::<f64>() {
                Ok(v) if *d != 0.0 => {
                    out.push_str(&num(v + d));
                    out.push_str(ending(l));
                }
                _ => out.push_str(l),
            }
        }
        i += 8;
    }
    (out, changed)
}

/// The lines of the `[object]` record with `id` in `text` - its own lines only, up to the next
/// keyword, their line endings included.
///
/// This is what a copy of that object is written from (see [`NewRecord::template_lines`]), and
/// it is taken from the text as it was opened, so a copy does not care whether the record is
/// still there by the time the file is written.
pub fn record_lines(text: &str, id: i64) -> Option<Vec<String>> {
    let lines: Vec<&str> = text.split_inclusive('\n').collect();
    let mut i = 0;
    while i < lines.len() {
        let is_object = body(lines[i]).trim().eq_ignore_ascii_case("[object]");
        let this = lines.get(i + 3).and_then(|l| body(l).trim().parse::<i64>().ok());
        if is_object && this == Some(id) {
            let start = i;
            i += 1;
            while i < lines.len() && !body(lines[i]).trim_start().starts_with('[') {
                i += 1;
            }
            return Some(lines[start..i].iter().map(|s| s.to_string()).collect());
        }
        i += 1;
    }
    None
}

/// One `[object]` record as a tile file writes it, read back for the editor's index.
#[derive(Clone, Debug, PartialEq)]
pub struct ObjectRecord {
    pub id: i64,
    pub file: String,
    pub pos: DVec3,
    pub heading: f64,
}

/// The `[object]` records a tile file holds, in the order they appear.
///
/// The record is read positionally - keyword, `0`, file, id, x, y, z, heading - which is
/// exactly the layout [`rewrite_tile`] and [`add_copies`] write back to. A record too short
/// to hold all of that is skipped rather than guessed at.
pub fn object_records(text: &str) -> Vec<ObjectRecord> {
    let lines: Vec<&str> = text.split_inclusive('\n').collect();
    let num = |k: usize| -> Option<f64> {
        lines.get(k).and_then(|l| body(l).trim().parse::<f64>().ok())
    };
    let mut out = Vec::new();
    for (i, line) in lines.iter().enumerate() {
        if !body(line).trim().eq_ignore_ascii_case("[object]") {
            continue;
        }
        let Some(id) = lines.get(i + 3).and_then(|l| body(l).trim().parse::<i64>().ok()) else { continue };
        let (Some(x), Some(y), Some(z), Some(heading)) = (num(i + 4), num(i + 5), num(i + 6), num(i + 7)) else { continue };
        out.push(ObjectRecord {
            id,
            file: body(lines[i + 2]).trim().to_string(),
            pos: DVec3::new(x, y, z),
            heading,
        });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const TILE: &str = "[version]\r\n4\r\n\r\n[object]\r\n0\r\nSceneryobjects\\a.sco\r\n7\r\n5\r\n6\r\n0.25\r\n90\r\n0\r\n0\r\n0\r\n\r\nObject Nr. 1\r\n[object]\r\n0\r\nSceneryobjects\\b.sco\r\n8\r\n1\r\n2\r\n0\r\n0\r\n0\r\n0\r\n2\r\nHalt\r\nx\r\n\r\n[spline]\r\n0\r\n";

    #[test]
    fn a_moved_object_changes_its_lines_and_a_deleted_one_goes() {
        let mut edits = HashMap::new();
        edits.insert(7, ObjectEdit { moved: DVec3::new(1.5, -1.0, 0.0), turned: 12.5, deleted: false });
        edits.insert(8, ObjectEdit { deleted: true, ..Default::default() });
        let (out, n) = rewrite_tile(TILE, &edits);
        assert_eq!(n, 2);
        assert!(out.contains("[object]\r\n0\r\nSceneryobjects\\a.sco\r\n7\r\n6.5\r\n5\r\n0.25\r\n102.5\r\n0\r\n"), "{out:?}");
        assert!(!out.contains("b.sco") && !out.contains("Halt"), "{out:?}");
        assert!(out.ends_with("[spline]\r\n0\r\n"));
    }

    /// The record lines of `id` in `text`, the way the document hands them to `add_copies`.
    fn record_of(text: &str, id: i64) -> Vec<String> {
        let lines: Vec<&str> = text.split_inclusive('\n').collect();
        let mut i = 0;
        while i < lines.len() {
            if body(lines[i]).trim().eq_ignore_ascii_case("[object]")
                && lines.get(i + 3).and_then(|l| body(l).trim().parse::<i64>().ok()) == Some(id)
            {
                let start = i;
                i += 1;
                while i < lines.len() && !body(lines[i]).trim_start().starts_with('[') {
                    i += 1;
                }
                return lines[start..i].iter().map(|s| s.to_string()).collect();
            }
            i += 1;
        }
        Vec::new()
    }

    /// Two objects, the first of them with a blank line after its record.
    const TWO: &str = "[object]\r\n0\r\nSceneryobjects\\A\\post.sco\r\n17\r\n10\r\n20\r\n0\r\n90\r\n\r\n[object]\r\n0\r\nx.sco\r\n18\r\n1\r\n1\r\n0\r\n0\r\n";

    fn copy_of_17(id: i64, file: &str) -> NewRecord {
        NewRecord {
            template: 17,
            id,
            file: file.into(),
            moved: DVec3::ZERO,
            turned: 0.0,
            template_lines: record_of(TWO, 17),
        }
    }

    #[test]
    fn a_copy_follows_its_template_with_its_own_id() {
        let mut c = copy_of_17(99, "lamp.sco");
        c.moved = DVec3::new(2.0, 0.0, 0.0);
        c.turned = 10.0;
        let (out, n) = add_copies(TWO, &[c]);
        assert_eq!(n, 1);
        assert!(out.contains("Sceneryobjects\\A\\lamp.sco\r\n99\r\n12\r\n20\r\n0\r\n100\r\n"), "{out}");
        assert!(out.contains("[object]\r\n0\r\nx.sco\r\n18"), "the other record went: {out}");
    }

    #[test]
    fn a_copy_whose_template_is_gone_is_still_written() {
        // object 17 was taken away by the same save, so its record is not in the file
        let removed = "[object]\r\n0\r\nx.sco\r\n18\r\n1\r\n1\r\n0\r\n0\r\n";
        let (out, n) = add_copies(removed, &[copy_of_17(99, "lamp.sco")]);
        assert_eq!(n, 1, "the copy was dropped without a word");
        assert!(out.contains("Sceneryobjects\\A\\lamp.sco\r\n99\r\n10\r\n20\r\n0\r\n90\r\n"), "{out}");
        assert!(out.contains("[object]\r\n0\r\nx.sco\r\n18"), "the other record went: {out}");
    }

    #[test]
    fn a_copy_with_no_template_record_is_not_written_at_all() {
        let mut c = copy_of_17(99, "lamp.sco");
        c.template_lines.clear();
        let (out, n) = add_copies(TWO, &[c]);
        assert_eq!(n, 0);
        assert_eq!(out, TWO);
    }

    /// What `Session::place_asset` hands to `add_copies`: an object out of the content folder,
    /// whose record is written from nothing and which has no template in the file to follow.
    #[test]
    fn a_record_written_from_nothing_goes_at_the_end_after_a_blank_line() {
        let fresh = NewRecord {
            template: 42,
            id: 42,
            file: "thing.sco".into(),
            moved: DVec3::ZERO,
            turned: 0.0,
            template_lines: blank_object_record("Sceneryobjects\\New\\thing.sco", 42, DVec3::new(3.0, 4.0, 0.0), 15.0),
        };
        let (out, n) = add_copies(TWO, &[fresh]);
        assert_eq!(n, 1);
        // the record says where the object stands, and stands where it says
        assert!(
            out.contains("[object]\r\n0\r\nSceneryobjects\\New\\thing.sco\r\n42\r\n3\r\n4\r\n0\r\n15\r\n0\r\n0\r\n0\r\n"),
            "{out:?}"
        );
        // a blank line between it and the record before it, as between every two records
        assert!(out.contains("0\r\n\r\n[object]\r\n0\r\nSceneryobjects\\New\\thing.sco"), "{out:?}");
        // and nothing that was in the file moved or changed
        assert!(out.starts_with(TWO), "{out:?}");
    }

    /// The lines of a record, as the file will read them back.
    #[test]
    fn a_blank_record_reads_back_as_the_object_it_was_written_for() {
        let lines = blank_object_record("Sceneryobjects\\New\\thing.sco", 7, DVec3::new(1.25, -3.0, 0.0), 270.0);
        let text: String = lines.concat();
        let got = object_records(&text);
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].id, 7);
        assert_eq!(got[0].file, "Sceneryobjects\\New\\thing.sco");
        assert_eq!(got[0].pos, DVec3::new(1.25, -3.0, 0.0));
        assert_eq!(got[0].heading, 270.0);
        // a forward slash is written the way a tile file writes a path
        let lines = blank_object_record("Sceneryobjects/New/thing.sco", 7, DVec3::ZERO, 0.0);
        assert_eq!(lines[2], "Sceneryobjects\\New\\thing.sco\r\n");
    }

    #[test]
    fn a_tile_with_nothing_to_do_comes_back_byte_for_byte() {
        assert_eq!(rewrite_tile(TILE, &HashMap::new()), (TILE.to_string(), 0));
    }

    #[test]
    fn an_untouched_object_edit_changes_nothing_either() {
        // a zero delta must not rewrite the number, or "5" could come back as "5.0000"
        let mut edits = HashMap::new();
        edits.insert(7, ObjectEdit::default());
        let (out, n) = rewrite_tile(TILE, &edits);
        assert_eq!(n, 1);
        assert_eq!(out, TILE);
    }

    #[test]
    fn the_records_a_tile_holds_are_read_in_order() {
        let r = object_records(TILE);
        assert_eq!(r.len(), 2);
        assert_eq!(r[0].id, 7);
        assert_eq!(r[0].file, "Sceneryobjects\\a.sco");
        assert_eq!(r[0].pos, DVec3::new(5.0, 6.0, 0.25));
        assert_eq!(r[0].heading, 90.0);
        assert_eq!(r[1].id, 8);
        assert_eq!(r[1].pos, DVec3::new(1.0, 2.0, 0.0));
    }

    #[test]
    fn an_attach_object_is_not_an_object() {
        assert!(object_records("[attachObj]\r\n0\r\np.sco\r\n9\r\n1\r\n2\r\n0\r\n0\r\n").is_empty());
    }

    #[test]
    fn a_record_too_short_to_hold_a_pose_is_skipped() {
        // no heading line: skipped rather than read as 0
        assert!(object_records("[object]\r\n0\r\np.sco\r\n9\r\n1\r\n2\r\n0\r\n").is_empty());
    }
}
