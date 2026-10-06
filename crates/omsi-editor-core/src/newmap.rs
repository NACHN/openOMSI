//! A map where there was none.
//!
//! Everything else in this crate opens a map that somebody else made. This is the one place
//! that writes the first version of one: a folder of its own under `maps/`, a `global.cfg`
//! that names it, one flat tile to stand on, and one entry point in the middle of that tile
//! so the map is opened somewhere with ground under it rather than at a corner of nowhere.
//!
//! What it writes is deliberately the least a map can be, and it is written the way OMSI's
//! own editor writes: UTF-16 little endian with a byte order mark, CRLF between the lines.
//! The formats are the ones this project already reads ([`omsi_map::GlobalCfg`],
//! [`omsi_map::Tile`]), so a map made here opens the same way a downloaded one does - and the
//! tests read one back to say so.
//!
//! Nothing here decides where a map is *edited*: the folder it makes is the map's own, and a
//! save then follows whatever [`Destination`](crate::Destination) the session was opened
//! with, which is the same rule for every map.

use crate::codec::{encode, Encoding};
use crate::EditError;
use omsi_map::Terrain;
use std::path::{Path, PathBuf};

/// The version OMSI 2's own editor writes into `global.cfg` and into a tile.
const MAP_VERSION: i32 = 14;

/// The one tile a new map has, and its file.
const FIRST_TILE: (i32, i32) = (0, 0);

/// How far a name may go - a folder name long enough to be a problem on every system.
const NAME_LIMIT: usize = 64;

/// A map that was just made: what it is called, and the `global.cfg` to open it by.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MadeMap {
    /// The name the folder got (the name asked for, trimmed).
    pub name: String,
    /// The map's folder.
    pub dir: PathBuf,
    /// `…/maps/<name>/global.cfg`.
    pub global: PathBuf,
}

/// Make a map called `name` under `content/maps/`, with one flat tile and one entry point.
///
/// `content` is the content folder - the one the game reads before the installation - and not
/// the OMSI 2 installation: a map is a person's own work from its first byte, and this crate
/// never writes inside the installation (see [`Destination`](crate::Destination)).
///
/// A map of that name already being there is an error rather than an overwrite: what is in a
/// map folder is somebody's afternoon, and no click should be able to lose it.
pub fn create(content: &Path, name: &str) -> Result<MadeMap, EditError> {
    let name = clean_name(name)?;
    let dir = content.join("maps").join(&name);
    let global = dir.join("global.cfg");
    if global.exists() {
        return Err(EditError::Other("A map of that name is already there".into()));
    }
    std::fs::create_dir_all(&dir).map_err(|e| EditError::io(&dir, e))?;
    // a folder that exists but is empty is nobody's map: leave it rather than refuse halfway
    let written = write_map(&dir, &name);
    if written.is_err() {
        // half a map is worse than none: it would open and be missing its ground
        let _ = std::fs::remove_dir_all(&dir);
    }
    written?;
    Ok(MadeMap { name, dir, global })
}

/// The name a map folder may have, or why it may not.
///
/// A map's name is a folder name, so it answers to what a folder answers to on every system
/// this runs on - and to two more rules of our own: it must not be empty, and it must not be
/// a name that a path could walk out of `maps/` with (`..`, or anything holding a separator).
///
/// Every refusal is one fixed sentence, with nothing of the name in it: the name that failed
/// is in the field it was typed in, right under where the refusal is said, and a sentence a
/// person reads twice should be one the interface can translate.
pub fn clean_name(name: &str) -> Result<String, EditError> {
    let name = name.trim();
    if name.is_empty() {
        return Err(EditError::Other("A map needs a name".into()));
    }
    if name.chars().any(|c| c.is_control() || "\\/:*?\"<>|".contains(c)) {
        return Err(EditError::Other("A map's name cannot hold a character a file name cannot".into()));
    }
    // (Windows drops these, and a folder called ".hidden" is not what a click meant)
    if name.starts_with('.') || name.ends_with('.') {
        return Err(EditError::Other("A map's name cannot start or end with a dot".into()));
    }
    if name.chars().count() > NAME_LIMIT {
        return Err(EditError::Other("A map's name must be at most 64 characters long".into()));
    }
    Ok(name.to_string())
}

/// The files of a map that has nothing in it yet.
fn write_map(dir: &Path, name: &str) -> Result<(), EditError> {
    write_utf16(&dir.join("global.cfg"), &global_text(name))?;
    write_utf16(&dir.join(tile_file(FIRST_TILE)), &EMPTY_TILE)?;
    // the ground: flat, and written the way OMSI writes a height field
    let terrain = dir.join(format!("{}.terrain", tile_file(FIRST_TILE)));
    std::fs::write(&terrain, Terrain::flat().to_bytes()).map_err(|e| EditError::io(&terrain, e))?;
    Ok(())
}

fn write_utf16(path: &Path, text: &str) -> Result<(), EditError> {
    std::fs::write(path, encode(text, Encoding::Utf16Le)).map_err(|e| EditError::io(path, e))
}

/// The tile file of a place on the map.
fn tile_file(tile: (i32, i32)) -> String {
    format!("tile_{}_{}.map", tile.0, tile.1)
}

/// A new map's `global.cfg`.
///
/// Only what a map cannot be read without: what it is called, the version its files are
/// written in, the next id to hand out, the one entry point, and the one tile. No money
/// system, no ticket pack, no ground textures and no traffic curves - those are a map's own
/// choices, and a new map has not made them yet (the game falls back on its own).
fn global_text(name: &str) -> String {
    let middle = omsi_map::tile_size() / 2.0;
    let (tx, ty) = FIRST_TILE;
    format!(
        "[name]\r\n{name}\r\n\r\n\
         [friendlyname]\r\n{name}\r\n\r\n\
         [version]\r\n{MAP_VERSION}\r\n\r\n\
         [NextIDCode]\r\n1\r\n\r\n\
         [entrypoints]\r\n1\r\n\
         0\r\n0\r\n0\r\n\
         {middle}\r\n0\r\n{middle}\r\n\
         0\r\n0\r\n0\r\n1\r\n\
         0\r\nStart\r\n\r\n\
         [map]\r\n{tx}\r\n{ty}\r\n{}\r\n",
        tile_file(FIRST_TILE)
    )
}

/// A tile with nothing in it but its version: no object, no spline, and the terrain beside it
/// rather than in it (which is where OMSI keeps the heights: `tile_0_0.map.terrain`).
pub(crate) const EMPTY_TILE: &str = "[version]\r\n14\r\n\r\n[terrain]\r\n\r\n\r\n[variable_terrainlightmap]\r\n\r\n[variable_terrain]\r\n\r\n";

#[cfg(test)]
mod tests {
    use super::*;
    use omsi_map::{GlobalCfg, Tile};

    /// A content folder of its own for one test.
    fn content(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("openomsi-newmap-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn a_new_map_reads_back_as_the_map_it_says_it_is() {
        let content = content("reads_back");
        let made = create(&content, "Emptiness").unwrap();
        assert_eq!(made.name, "Emptiness");
        assert!(made.global.exists());

        // the name, the one tile, the one entry point
        let g = GlobalCfg::load(&made.global).unwrap();
        assert_eq!((g.name.as_str(), g.friendly_name.as_str()), ("Emptiness", "Emptiness"));
        assert_eq!(g.version, MAP_VERSION);
        assert_eq!(g.tiles.len(), 1);
        assert_eq!((g.tiles[0].x, g.tiles[0].y), FIRST_TILE);
        assert_eq!(g.tiles[0].file, "tile_0_0.map");
        assert_eq!(g.entry_points.len(), 1);
        // the entry point stands in the middle of the tile, on its ground
        let ep = &g.entry_points[0];
        assert_eq!(ep.pos, [omsi_map::tile_size() / 2.0, omsi_map::tile_size() / 2.0, 0.0]);

        // and the tile it names is there, empty, and readable
        let tile = Tile::load(&made.dir.join("tile_0_0.map")).unwrap();
        assert!(tile.objects.is_empty());
        assert!(tile.splines.is_empty());
        assert_eq!(tile.version, MAP_VERSION);

        // the ground is flat, and as wide as a tile
        let t = Terrain::load(&made.dir.join("tile_0_0.map.terrain")).unwrap();
        assert_eq!(t.samples(), omsi_map::TERRAIN_SAMPLES);
        assert!(t.heights.iter().all(|h| *h == 0.0));
    }

    #[test]
    fn the_files_are_written_the_way_omsi_writes_them() {
        let content = content("utf16");
        let made = create(&content, "Encoding").unwrap();
        for file in ["global.cfg", "tile_0_0.map"] {
            let bytes = std::fs::read(made.dir.join(file)).unwrap();
            assert_eq!(&bytes[..2], &[0xFF, 0xFE], "{file} has no byte order mark");
            // every odd byte of a UTF-16LE file outside the ASCII range is 0
            assert!(bytes[2..].chunks_exact(2).any(|c| c[1] == 0), "{file} is not UTF-16LE");
            // (and CRLF, which is what OMSI's own editor writes)
            assert!(bytes.windows(2).any(|w| w == [b'\r', 0]), "{file} has no CR");
        }
    }

    #[test]
    fn a_map_that_is_already_there_is_not_overwritten() {
        let content = content("twice");
        let made = create(&content, "Twice").unwrap();
        std::fs::write(made.dir.join("tile_0_0.map"), "somebody's afternoon").unwrap();
        let again = create(&content, "Twice");
        assert!(again.is_err(), "a second map of the same name was made");
        // and what was there is still there
        assert_eq!(std::fs::read_to_string(made.dir.join("tile_0_0.map")).unwrap(), "somebody's afternoon");
    }

    #[test]
    fn a_name_that_is_not_a_folder_name_is_refused() {
        assert!(clean_name("  ").is_err());
        assert!(clean_name("").is_err());
        assert!(clean_name("a/b").is_err());
        assert!(clean_name("a\\b").is_err());
        assert!(clean_name("..").is_err());
        assert!(clean_name(".hidden").is_err());
        assert!(clean_name("a:b").is_err());
        assert!(clean_name(&"x".repeat(NAME_LIMIT + 1)).is_err());
        assert_eq!(clean_name("  Mitte  ").unwrap(), "Mitte");
    }

    #[test]
    fn a_map_that_cannot_be_written_leaves_nothing_behind() {
        let content = content("half");
        // a file where the map's folder would go: the folder cannot be made
        std::fs::create_dir_all(content.join("maps")).unwrap();
        std::fs::write(content.join("maps").join("Blocked"), "not a folder").unwrap();
        assert!(create(&content, "Blocked").is_err());
        assert!(content.join("maps").join("Blocked").is_file(), "what was there was disturbed");
    }
}
