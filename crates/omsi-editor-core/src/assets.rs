//! The `.sco` files a map can be built from: what the folders an object's record names hold.
//!
//! Everything else in this crate works on an object that is already somewhere - a record in a
//! tile file, a copy of one. This is the one list that comes from the other direction: the
//! content folder walked for scenery objects, so that an object that is in **no** map yet can
//! be put into one (see [`Session::place_asset`](crate::Session::place_asset)).
//!
//! What comes out is what a record would have written in it: `Sceneryobjects\Berlin\House1.sco`
//! - the spelling, backslashes and all, that a tile file holds and that
//! [`resolve_path`](omsi_cfg::resolve_path) turns back into a file on this machine. The paths
//! are relative to the folder holding `maps/`, which is what
//! [`Document::install_root`](crate::Document::install_root) answers.

use std::collections::BTreeSet;
use std::path::Path;

/// The content folder scenery objects live under, as a record writes it.
const SCENERY: &str = "Sceneryobjects";

/// How deep the walk goes. Objects sit two or three folders down
/// (`Sceneryobjects\Pack\Sub\thing.sco`); the limit is there so that a folder joined to itself
/// - a junction or a link, which a content folder is free to hold - cannot make this run
/// forever.
const DEPTH_LIMIT: usize = 8;

/// Every `.sco` file under `root`'s `Sceneryobjects`, as a record would name it, sorted and
/// each once.
///
/// `root` is the folder that holds `maps/` and `Sceneryobjects` - the installation, or the
/// content folder a map came from. A mod that adds objects to a stock folder puts them into
/// the same folder under the content folder, so both are walked: the listing goes through
/// [`mirrored_dirs`](omsi_cfg::mirrored_dirs), which is the rule every other read of a content
/// file follows.
///
/// It lists files, not what can be read: a `.sco` this program cannot parse is still something
/// a map may name, and refusing to offer it would refuse a map OMSI itself accepts.
pub fn scenery_objects(root: &Path) -> Vec<String> {
    let mut out: BTreeSet<String> = BTreeSet::new();
    let base = root.join(SCENERY);
    for dir in omsi_cfg::mirrored_dirs(&base) {
        walk(&dir, SCENERY, 0, &mut out);
    }
    out.into_iter().collect()
}

/// Every `.sco` under `dir`, each named with `prefix` in front of it.
fn walk(dir: &Path, prefix: &str, depth: usize, out: &mut BTreeSet<String>) {
    if depth > DEPTH_LIMIT {
        log::warn!("{}: not looking any deeper for scenery objects", dir.display());
        return;
    }
    // through the vfs, so that an object inside an archive in `Archives` is listed beside the
    // ones lying on disk
    let Some(entries) = omsi_cfg::vfs::list_dir(dir) else { return };
    for (name, is_dir) in entries {
        let name = name.to_string_lossy().to_string();
        let rel = format!("{prefix}\\{name}");
        if is_dir {
            walk(&dir.join(&name), &rel, depth + 1, out);
        } else if name.to_ascii_lowercase().ends_with(".sco") {
            out.insert(rel);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    /// A throwaway content folder with a few objects in it.
    fn root(name: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!("openomsi-assets-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        for rel in [
            "Sceneryobjects/Berlin/House1.sco",
            "Sceneryobjects/Berlin/House2.sco",
            "Sceneryobjects/Streetobjects/Lamp.sco",
            "Sceneryobjects/Streetobjects/Lamp.o3d",
            "Sceneryobjects/Trees/Deciduous/Oak.sco",
        ] {
            let p = root.join(rel);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(p, "[mesh]\r\nx.o3d\r\n").unwrap();
        }
        root
    }

    #[test]
    fn every_sco_is_listed_once_named_the_way_a_record_names_one() {
        let root = root("listed");
        let got = scenery_objects(&root);
        assert_eq!(
            got,
            vec![
                "Sceneryobjects\\Berlin\\House1.sco".to_string(),
                "Sceneryobjects\\Berlin\\House2.sco".to_string(),
                "Sceneryobjects\\Streetobjects\\Lamp.sco".to_string(),
                "Sceneryobjects\\Trees\\Deciduous\\Oak.sco".to_string(),
            ]
        );
        // the model beside a lamp is not a scenery object
        assert!(!got.iter().any(|s| s.ends_with(".o3d")), "{got:?}");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_folder_that_is_not_there_lists_nothing_and_is_not_an_error() {
        let missing = std::env::temp_dir().join("openomsi-assets-none-such-folder");
        assert!(scenery_objects(&missing).is_empty());
    }

    #[test]
    fn a_record_written_from_a_listed_name_lands_in_that_file() {
        // the whole point of the spelling: what the list hands out is what a lookup resolves
        let root = root("resolves");
        let sco = "Sceneryobjects\\Berlin\\House1.sco";
        assert!(scenery_objects(&root).contains(&sco.to_string()));
        let found = omsi_cfg::resolve_path(&root, sco);
        assert!(omsi_cfg::vfs::is_file(&found), "{}", found.display());
        let _ = std::fs::remove_dir_all(&root);
    }
}
