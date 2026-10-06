//! `global.cfg` edited as text: a tile added to the map's list of them, or taken out of it.
//!
//! The map's own `[map]` list is what says which tiles exist, so making a tile and taking one
//! away are both edits to that list - and they are not the same size of edit.
//!
//! **Adding one is an append.** A keyword and three lines go after the last of them and not
//! one byte already in the file moves. Nothing can be pointing at a number that changed,
//! because no number changed.
//!
//! **Taking one out is not.** An entry point names its tile by that tile's *place in this
//! list* (`EntryPoint::group`, which is an index into `GlobalCfg::raw_tiles`), and so does a
//! timetable track's `[track_entry]`. Removing an entry from the middle therefore renumbers
//! every entry after it, and everything that names a later tile goes on naming the same
//! number - which is now a different tile. That is a map quietly rewritten to mean something
//! else, which is the one thing a map editor must never do, so:
//!
//! - an entry point's number is rewritten in the same pass, because it is in this file. An
//!   entry point standing *in* the tile that goes is refused instead: it is where the player
//!   starts, and there is nothing sensible to move it to;
//! - a track file is not in this file, and this project does not write `.ttr`, so
//!   [`can_remove`] reports `Renumbers` and the caller refuses that when the map has any track
//!   file at all. Saying so is the point: a route pointing at the wrong tile is a bug nobody
//!   would find by looking at the map.
//!
//! Everything else in `global.cfg` - comments, unknown keywords, the order of the blocks, the
//! line endings, the encoding - is passed through byte for byte, exactly as `record.rs` does
//! for a tile's `[object]` records.

use crate::codec::{body, ending};
use std::ops::Range;

/// One `[map]` entry of `global.cfg`: where it says the tile is, and the lines it takes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MapEntry {
    pub x: i32,
    pub y: i32,
    pub file: String,
    /// The `[map]` line and the three lines under it: x, y, and the tile's file.
    pub lines: Range<usize>,
}

/// Every `[map]` entry of `global.cfg`, in the order the file lists them.
///
/// The order is not cosmetic. An entry point names its tile by its place in this list, and a
/// track file names one the same way, so this is the numbering the rest of the map counts in -
/// including the entries a map lists twice, which count as two (see `GlobalCfg::raw_tiles`).
pub fn map_entries(text: &str) -> Vec<MapEntry> {
    let lines: Vec<&str> = text.split_inclusive('\n').collect();
    let mut out = Vec::new();
    for i in 0..lines.len() {
        if !body(lines[i]).trim().eq_ignore_ascii_case("[map]") {
            continue;
        }
        let num = |k: usize| lines.get(k).and_then(|l| body(l).trim().parse::<i32>().ok());
        let (Some(x), Some(y)) = (num(i + 1), num(i + 2)) else { continue };
        let file = lines.get(i + 3).map(|l| body(l).trim().to_string()).unwrap_or_default();
        out.push(MapEntry { x, y, file, lines: i..(i + 4).min(lines.len()) });
    }
    out
}

/// `global.cfg` with an entry for the tile at `x, y` added after the last one.
///
/// An append and nothing else: every entry already in the list keeps its place, so every
/// number that names one still names the same tile. The new entry goes directly after the last
/// `[map]` block, so the list stays together in the file, and takes the file's own line ending.
pub fn add_entry(text: &str, x: i32, y: i32, file: &str) -> String {
    let lines: Vec<&str> = text.split_inclusive('\n').collect();
    let entries = map_entries(text);
    let eol = entries
        .last()
        .and_then(|e| lines.get(e.lines.start))
        .map(|l| ending(l).to_string())
        .filter(|e| !e.is_empty())
        .or_else(|| lines.iter().find(|l| !ending(l).is_empty()).map(|l| ending(l).to_string()))
        .unwrap_or_else(|| "\r\n".to_string());
    let at = entries.last().map(|e| e.lines.end).unwrap_or(lines.len());

    let mut out = String::with_capacity(text.len() + 64);
    for l in &lines[..at] {
        out.push_str(l);
    }
    // a blank line between blocks, as the rest of the file has
    if !out.is_empty() && !out.ends_with(&format!("{eol}{eol}")) {
        out.push_str(&eol);
    }
    out.push_str(&format!("[map]{eol}{x}{eol}{y}{eol}{file}{eol}"));
    for l in &lines[at..] {
        out.push_str(l);
    }
    out
}

/// Why the `[map]` entry at `index` cannot be taken out of the list.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Removal {
    /// It can, and nothing that names a tile is left pointing at the wrong one.
    Fine,
    /// There is no `[map]` entry at that place.
    NoSuchEntry,
    /// An entry point stands in that tile - it is where the player starts - so there is
    /// nothing sensible to move it to.
    EntryPointStands,
    /// Every entry after this one changes its number. An entry point's number *is* rewritten
    /// by [`remove_entry`], but a track file's is not - so a caller refuses this when the map
    /// has one.
    Renumbers,
}

/// Whether the `[map]` entry at `index` can be taken out, and if it cannot, why.
pub fn can_remove(text: &str, index: usize) -> Removal {
    let entries = map_entries(text);
    if index >= entries.len() {
        return Removal::NoSuchEntry;
    }
    if entry_point_groups(text).iter().any(|g| *g == index as i32) {
        return Removal::EntryPointStands;
    }
    if index + 1 < entries.len() {
        return Removal::Renumbers;
    }
    Removal::Fine
}

/// `global.cfg` with the `[map]` entry at `index` gone and every entry point that named a
/// later tile renumbered to name the same tile it named before.
///
/// The entry goes with the blank line that separates it from the next block, the way a deleted
/// `[object]` record goes with what follows it (`record.rs`) - so the file does not gather
/// empty lines where entries used to be.
///
/// Call it only on an index [`can_remove`] did not refuse; on an index out of range it hands
/// the text back unchanged.
pub fn remove_entry(text: &str, index: usize) -> String {
    let lines: Vec<&str> = text.split_inclusive('\n').collect();
    let entries = map_entries(text);
    let Some(entry) = entries.get(index) else { return text.to_string() };
    let mut end = entry.lines.end;
    if lines.get(end).is_some_and(|l| body(l).trim().is_empty()) {
        end += 1;
    }

    let mut out = String::with_capacity(text.len());
    for (i, l) in lines.iter().enumerate() {
        if i >= entry.lines.start && i < end {
            continue;
        }
        out.push_str(l);
    }
    shift_groups(&out, index as i32 + 1, -1)
}

/// `global.cfg` with a `[map]` entry for the tile at `x, y` put back at `at` - the other half
/// of [`remove_entry`].
///
/// The entry goes back where it was so that every number naming a tile means what it meant
/// before, and every entry point that named a tile at or after that place moves up one to go
/// on naming the same tile. `at` past the end of the list appends.
pub fn insert_entry(text: &str, at: usize, x: i32, y: i32, file: &str) -> String {
    let entries = map_entries(text);
    if at >= entries.len() {
        return add_entry(text, x, y, file);
    }
    let lines: Vec<&str> = text.split_inclusive('\n').collect();
    let start = entries[at].lines.start;
    let eol = ending(lines[start]);
    let eol = if eol.is_empty() { "\r\n" } else { eol };

    let mut out = String::with_capacity(text.len() + 64);
    for l in &lines[..start] {
        out.push_str(l);
    }
    out.push_str(&format!("[map]{eol}{x}{eol}{y}{eol}{file}{eol}{eol}"));
    for l in &lines[start..] {
        out.push_str(l);
    }
    shift_groups(&out, at as i32, 1)
}

/// Every entry point's tile number, in the order the file lists them.
pub fn entry_point_groups(text: &str) -> Vec<i32> {
    let lines: Vec<&str> = text.split_inclusive('\n').collect();
    entry_point_blocks(&lines)
        .into_iter()
        .filter_map(|at| lines.get(at).and_then(|l| body(l).trim().parse::<i32>().ok()))
        .collect()
}

/// `text` with every entry point's tile number at or after `from` moved by `shift`, so that
/// a number goes on naming the same tile after the list has gained or lost an entry.
fn shift_groups(text: &str, from: i32, shift: i32) -> String {
    let lines: Vec<&str> = text.split_inclusive('\n').collect();
    let mut out = String::with_capacity(text.len());
    let mut i = 0;
    while i < lines.len() {
        out.push_str(lines[i]);
        i += 1;
        if !body(lines[i - 1]).trim().eq_ignore_ascii_case("[entrypoints]") {
            continue;
        }
        let Some(n) = lines.get(i).and_then(|l| body(l).trim().parse::<usize>().ok()) else { continue };
        out.push_str(lines[i]);
        i += 1;
        for _ in 0..n {
            if i + ENTRY_LINES > lines.len() {
                break;
            }
            for l in &lines[i..i + GROUP_LINE] {
                out.push_str(l);
            }
            match body(lines[i + GROUP_LINE]).trim().parse::<i32>() {
                Ok(g) if g >= from => {
                    out.push_str(&(g + shift).to_string());
                    out.push_str(ending(lines[i + GROUP_LINE]));
                }
                _ => out.push_str(lines[i + GROUP_LINE]),
            }
            for l in &lines[i + GROUP_LINE + 1..i + ENTRY_LINES] {
                out.push_str(l);
            }
            i += ENTRY_LINES;
        }
    }
    out
}

/// An entry point is written as twelve lines: its index, its object, an unknown number, x, z,
/// y, four quaternion numbers, the number of the tile it stands in, and its name (see
/// `omsi_map`'s parser).
const ENTRY_LINES: usize = 12;
/// How many of those come before the tile's number.
const GROUP_LINE: usize = 10;

/// Where each entry point's twelve lines start.
fn entry_point_blocks(lines: &[&str]) -> Vec<usize> {
    let mut out = Vec::new();
    for (i, l) in lines.iter().enumerate() {
        if !body(l).trim().eq_ignore_ascii_case("[entrypoints]") {
            continue;
        }
        let Some(n) = lines.get(i + 1).and_then(|l| body(l).trim().parse::<usize>().ok()) else { continue };
        for k in 0..n {
            let at = i + 2 + k * ENTRY_LINES;
            if at + ENTRY_LINES > lines.len() {
                break;
            }
            out.push(at + GROUP_LINE);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Three tiles, one entry point, and a block that follows the list - so a test can say
    /// that everything outside the edit is untouched. The entry point stands in the third
    /// tile, which is what makes a removal from the front of the list renumber something.
    const MAP: &str = "[name]\r\nMitte\r\n\r\n\
                       [version]\r\n14\r\n\r\n\
                       [map]\r\n0\r\n0\r\ntile_0_0.map\r\n\r\n\
                       [map]\r\n1\r\n0\r\ntile_1_0.map\r\n\r\n\
                       [map]\r\n2\r\n0\r\ntile_2_0.map\r\n\r\n\
                       [entrypoints]\r\n1\r\n\
                       0\r\n0\r\n0\r\n150\r\n0\r\n450\r\n0\r\n0\r\n0\r\n1\r\n2\r\nFar\r\n\r\n\
                       [splineobjtypes]\r\nSplines\\road.sli\r\n";

    /// Two tiles with the entry point in the first: the second can go, and nothing numbers it.
    const TWO: &str = "[map]\r\n0\r\n0\r\ntile_0_0.map\r\n\r\n\
                       [map]\r\n1\r\n0\r\ntile_1_0.map\r\n\r\n\
                       [entrypoints]\r\n1\r\n\
                       0\r\n0\r\n0\r\n150\r\n0\r\n150\r\n0\r\n0\r\n0\r\n1\r\n0\r\nStart\r\n";

    #[test]
    fn the_entries_are_read_where_the_file_puts_them() {
        let e = map_entries(MAP);
        assert_eq!(e.len(), 3);
        assert_eq!((e[0].x, e[0].y, e[0].file.as_str()), (0, 0, "tile_0_0.map"));
        assert_eq!((e[2].x, e[2].y, e[2].file.as_str()), (2, 0, "tile_2_0.map"));
        // and the entry point names its tile by that tile's place in the list
        assert_eq!(entry_point_groups(MAP), vec![2]);
    }

    #[test]
    fn an_added_tile_goes_after_the_last_one_and_disturbs_nothing_else() {
        let out = add_entry(MAP, 3, 0, "tile_3_0.map");
        let e = map_entries(&out);
        assert_eq!(e.len(), 4);
        assert_eq!((e[3].x, e[3].y, e[3].file.as_str()), (3, 0, "tile_3_0.map"));
        // the file is the old one with one block inserted, and nothing else at all
        let inserted = "\r\n[map]\r\n3\r\n0\r\ntile_3_0.map\r\n";
        assert!(out.contains(inserted), "{out:?}");
        assert_eq!(out.replace(inserted, ""), MAP);
        // every number that names a tile still names the same tile
        assert_eq!(entry_point_groups(&out), vec![2]);
        // and the new entry landed in the list, not after the block below it
        assert!(out.find("tile_3_0.map").unwrap() < out.find("[entrypoints]").unwrap());
    }

    #[test]
    fn the_last_tile_can_go_and_nothing_is_renumbered() {
        assert_eq!(can_remove(TWO, 1), Removal::Fine);
        let out = remove_entry(TWO, 1);
        assert_eq!(map_entries(&out).len(), 1);
        assert_eq!(map_entries(&out)[0].file, "tile_0_0.map");
        assert_eq!(entry_point_groups(&out), vec![0]);
        // only that block went: the rest of the file is what it was
        assert_eq!(out, TWO.replace("[map]\r\n1\r\n0\r\ntile_1_0.map\r\n\r\n", ""));
    }

    #[test]
    fn removing_a_middle_tile_moves_the_entry_point_that_named_a_later_one() {
        assert_eq!(can_remove(MAP, 1), Removal::Renumbers);
        let out = remove_entry(MAP, 1);
        assert_eq!(map_entries(&out).len(), 2);
        // the tile at 2,0 is now the second entry, so the entry point has to say 1
        assert_eq!(entry_point_groups(&out), vec![1]);
        // and the tile the entry point stands in is the one it stood in
        let tiles = map_entries(&out);
        assert_eq!(tiles[entry_point_groups(&out)[0] as usize].file, "tile_2_0.map");
    }

    #[test]
    fn an_entry_point_standing_in_the_tile_refuses_the_removal() {
        // the entry point is in the third tile: that tile cannot go
        assert_eq!(can_remove(MAP, 2), Removal::EntryPointStands);
        assert_eq!(remove_entry_over(can_remove(MAP, 2)), None);
    }

    #[test]
    fn an_index_that_is_not_there_is_refused_and_changes_nothing() {
        assert_eq!(can_remove(MAP, 9), Removal::NoSuchEntry);
        assert_eq!(remove_entry(MAP, 9), MAP);
    }

    #[test]
    fn a_file_with_line_feeds_only_is_edited_the_same_way() {
        let lf = MAP.replace("\r\n", "\n");
        let out = add_entry(&lf, 3, 0, "tile_3_0.map");
        assert!(!out.contains('\r'), "an LF file came back with CRLF lines");
        assert_eq!(map_entries(&out).len(), 4);
        assert_eq!(out.replace("\n[map]\n3\n0\ntile_3_0.map\n", ""), lf);
        assert_eq!(entry_point_groups(&out), vec![2]);
    }

    #[test]
    fn a_map_with_no_tile_list_at_all_takes_one() {
        let text = "[name]\r\nEmpty\r\n\r\n[version]\r\n14\r\n";
        let out = add_entry(text, 3, 4, "tile_3_4.map");
        assert_eq!(map_entries(&out).len(), 1);
        assert_eq!(out.replace("\r\n[map]\r\n3\r\n4\r\ntile_3_4.map\r\n", ""), text);
    }

    #[test]
    fn a_block_this_project_does_not_know_is_left_where_it_is() {
        let text = format!("[unknownthing]\r\nwhatever\r\n\r\n{MAP}");
        let out = add_entry(&text, 3, 0, "tile_3_0.map");
        assert!(out.starts_with("[unknownthing]\r\nwhatever\r\n\r\n"));
        assert_eq!(map_entries(&out).len(), 4);
        assert_eq!(entry_point_groups(&out), vec![2]);
    }

    #[test]
    fn putting_a_tile_back_where_it_was_undoes_the_removal_exactly() {
        for index in [0usize, 1] {
            let gone = remove_entry(MAP, index);
            let back = insert_entry(&gone, index, index as i32, 0, &format!("tile_{index}_0.map"));
            assert_eq!(back, MAP, "index {index} did not come back as it was");
        }
    }

    #[test]
    fn putting_a_tile_back_past_the_end_is_just_an_append() {
        let back = insert_entry(TWO, 9, 1, 0, "tile_1_0.map");
        assert_eq!(map_entries(&back).len(), 3);
        assert_eq!(map_entries(&back)[2].file, "tile_1_0.map");
        assert_eq!(entry_point_groups(&back), vec![0]);
    }

    /// What a caller does with a refusal: nothing, and the text is not touched.
    fn remove_entry_over(verdict: Removal) -> Option<String> {
        match verdict {
            Removal::Fine => Some("would have removed".into()),
            _ => None,
        }
    }
}
