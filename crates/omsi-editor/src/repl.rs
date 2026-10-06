//! The editor's command language: one line in, one change made.
//!
//! This is the editor's front end for a terminal. It is deliberately thin - it parses a line,
//! calls [`Session`], and prints what came back. Everything about *what an edit is* lives in
//! `omsi-editor-core`, which is the same code the window will drive, so the two can not
//! disagree about what "flatten" means or what an undo takes back.

use anyhow::{bail, Result};
use glam::DVec3;
use omsi_editor_core::{tile_file_name, Destination, Session};

/// Whether the session should carry on reading lines.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Flow {
    Continue,
    Quit,
}

/// What one line did.
#[derive(Debug)]
pub struct Outcome {
    /// What to say about it.
    pub message: String,
    pub flow: Flow,
}

impl Outcome {
    fn said(message: impl Into<String>) -> Outcome {
        Outcome { message: message.into(), flow: Flow::Continue }
    }

    fn quiet() -> Outcome {
        Outcome { message: String::new(), flow: Flow::Continue }
    }
}

/// What `help` prints.
pub const HELP: &str = "\
  info                     the map, the session, whether it is saved
  ls [x y] [text]          list objects, of one tile or matching a text
  sel <id>                 select an object by its id
  where                    where the selection stands
  variants                 the .sco files sitting beside the selection

  tiles                    the map's tiles, in the order its own list has them
  tile add <x> <y>         make the tile there, and add it to the map
  tile rm <x> <y>          take that tile out of the map (its file is left alone)

  mv <dx> <dy> <dz>        move the selection (m)
  move <x> <y> <z>         move the selection to a place (m)
  turn <deg>               turn the selection
  face <deg>               turn the selection to a heading
  del | undel              take the selection out of the map / put it back
  reset                    put everything done to the selection back
  place [name.sco] [x y z] a copy of the selection, there or two metres to its right
  assets [text]            the .sco files the content folder holds, matching a text
  new <sco> <x> <y> [h]    a brand-new object out of that folder, there, facing h degrees
  arm <sco> | arm off      hold a .sco in the place tool: a click then puts one down

  brush <r> | brush x1.25  the ground brush's radius (m, 1 to 60)
  raise <m> [x y]          raise the ground under the selection, or at x y
  lower <m> [x y]          lower it
  flatten [x y]            bring it to the height it already has there
  flatten-to <h> [x y]     bring it to a height

  undo | redo | history    take back, do again, what has been done
  save                     write the map (a save is a checkpoint: it clears the history)
  help | quit";

/// Run one line.
pub fn run_line(session: &mut Session, line: &str) -> Result<Outcome> {
    let line = line.trim();
    if line.is_empty() || line.starts_with('#') {
        return Ok(Outcome::quiet());
    }
    let mut words = line.split_whitespace();
    let cmd = words.next().unwrap_or("").to_ascii_lowercase();
    let rest: Vec<&str> = words.collect();

    match cmd.as_str() {
        "help" | "?" => Ok(Outcome::said(HELP)),

        "info" | "i" => Ok(Outcome::said(info(session))),

        "ls" | "list" | "objects" => Ok(Outcome::said(list(session, &rest)?)),

        "sel" | "select" => {
            let id = need_i64(&rest, 0, "sel <id>")?;
            session.select(Some(id));
            if session.selection().is_none() {
                bail!("no object {id} in this map");
            }
            Ok(Outcome::said(session.selection_summary()))
        }

        "where" => Ok(Outcome::said(session.selection_summary())),

        "tiles" | "tl" => Ok(Outcome::said(tiles(session))),

        "tile" => {
            let what = rest.first().copied().unwrap_or("").to_ascii_lowercase();
            let usage = "tile add <x> <y> | tile rm <x> <y> | tiles";
            match what.as_str() {
                "add" | "+" | "new" => {
                    let tile = need_tile(&rest, usage)?;
                    session.add_tile(tile)?;
                    Ok(Outcome::said(one_tile(session, tile, "added")))
                }
                "rm" | "remove" | "del" | "-" => {
                    let tile = need_tile(&rest, usage)?;
                    session.remove_tile(tile)?;
                    Ok(Outcome::said(format!("Tile ({}, {}) is out of the map. Its own file is still on disk.", tile.0, tile.1)))
                }
                _ => bail!("{usage}"),
            }
        }

        "variants" => {
            let v = session.sibling_variants();
            Ok(Outcome::said(if v.is_empty() { "No .sco file sits beside it".into() } else { v.join("\n") }))
        }

        "mv" => with_selection(session, |s| {
            let d = need_vec3(&rest, 0, "mv <dx> <dy> <dz>")?;
            Ok(s.move_selected(d)?)
        }),

        "move" | "at" => with_selection(session, |s| {
            let p = need_vec3(&rest, 0, "move <x> <y> <z>")?;
            Ok(s.move_selected_to(p)?)
        }),

        "turn" => with_selection(session, |s| {
            let d = need_f64(&rest, 0, "turn <deg>")?;
            Ok(s.turn_selected(d)?)
        }),

        "face" => with_selection(session, |s| {
            let h = need_f64(&rest, 0, "face <deg>")?;
            Ok(s.turn_selected_to(h)?)
        }),

        "del" | "delete" | "rm" => with_selection(session, |s| Ok(s.set_selected_deleted(true)?)),

        "undel" => with_selection(session, |s| Ok(s.set_selected_deleted(false)?)),

        "reset" => with_selection(session, |s| Ok(s.reset_selected()?)),

        "place" => {
            let template = current(session)?;
            with_selection(session, |s| {
                // an optional .sco name, then an optional place
                let (name, nums) = match rest.first() {
                    Some(w) if w.to_ascii_lowercase().ends_with(".sco") => (Some(w.to_string()), &rest[1..]),
                    _ => (None, &rest[..]),
                };
                let at = if nums.is_empty() {
                    None
                } else {
                    Some(need_vec3(nums, 0, "place [name.sco] [x y z]")?)
                };
                Ok(s.place_copy(template, name, at, 0.0)?)
            })
        }

        // the one command that is not about what is in the map: `.sco` files the content
        // folder holds, which no map needs to have heard of yet
        "assets" | "asset" => Ok(Outcome::said(assets(session, &rest.join(" ")))),

        "new" => {
            let usage = "new <sco> <x> <y> [heading]";
            let sco = rest.first().copied().ok_or_else(|| anyhow::anyhow!("{usage}"))?.to_string();
            let x = need_f64(&rest, 1, usage)?;
            let y = need_f64(&rest, 2, usage)?;
            let heading = match rest.get(3) {
                Some(w) => w.parse::<f64>().map_err(|_| anyhow::anyhow!("{usage}"))?,
                None => 0.0,
            };
            match session.place_asset(&sco, DVec3::new(x, y, 0.0), heading)? {
                Some(l) => Ok(Outcome::said(l)),
                None => Ok(Outcome::said("No change")),
            }
        }

        "arm" => {
            let what = rest.first().copied().unwrap_or("");
            if what.is_empty() || ["off", "none", "clear", "-"].contains(&what.to_ascii_lowercase().as_str()) {
                session.arm_asset(None);
                return Ok(Outcome::said("The place tool holds nothing: a click copies the selection."));
            }
            if !what.to_ascii_lowercase().ends_with(".sco") {
                bail!("arm <sco> | arm off");
            }
            session.arm_asset(Some(what.to_string()));
            Ok(Outcome::said(format!("The place tool holds {what} - a click puts one down.")))
        }

        "brush" => {
            let w = rest.first().copied().unwrap_or("");
            let r = match w.strip_prefix('x').or_else(|| w.strip_prefix('*')) {
                Some(f) => session.scale_brush(f.parse().map_err(|_| anyhow::anyhow!("brush x<factor>, e.g. brush x1.25"))?),
                None => session.set_brush(need_f64(&rest, 0, "brush <r>")?),
            };
            Ok(Outcome::said(format!("Brush {r:.1} m")))
        }

        "raise" | "lower" => {
            let m = need_f64(&rest, 0, "raise <m> [x y]")? * if cmd == "lower" { -1.0 } else { 1.0 };
            let at = ground_point(session, &rest[1..])?;
            let out = session.raise_ground(at, m)?;
            Ok(Outcome::said(out.unwrap_or_else(|| "Nothing in reach".into())))
        }

        "flatten" | "flatten-to" => {
            let (to, nums) = if cmd == "flatten-to" {
                (Some(need_f64(&rest, 0, "flatten-to <h> [x y]")?), &rest[1..])
            } else {
                (None, &rest[..])
            };
            let at = ground_point(session, nums)?;
            let out = session.flatten_ground(at, to)?;
            Ok(Outcome::said(out.unwrap_or_else(|| "Nothing in reach".into())))
        }

        "undo" | "u" => Ok(Outcome::said(match session.undo()? {
            Some(l) => format!("Undone: {l}"),
            None => "Nothing to take back".into(),
        })),

        "redo" | "r" => Ok(Outcome::said(match session.redo()? {
            Some(l) => format!("Redone: {l}"),
            None => "Nothing to do again".into(),
        })),

        "history" | "hist" => {
            let out: Vec<String> = session.history().entries().enumerate().map(|(i, c)| format!("{:>4}  {}", i + 1, c.label())).collect();
            Ok(Outcome::said(if out.is_empty() { "Nothing done yet".into() } else { out.join("\n") }))
        }

        "save" | "w" => Ok(Outcome::said(session.save()?.describe())),

        "quit" | "q" | "exit" => Ok(Outcome { message: String::new(), flow: Flow::Quit }),

        _ => bail!("no such command: {cmd} (help for the list)"),
    }
}

/// The selected object's id.
fn current(session: &Session) -> Result<i64> {
    session.selected_id().ok_or_else(|| anyhow::anyhow!("nothing is selected"))
}

/// Run a closure that needs something selected. The closure reports back through `anyhow`, so
/// that a parse mistake and a refused edit read the same way to the person typing.
fn with_selection(session: &mut Session, f: impl FnOnce(&mut Session) -> Result<Option<String>>) -> Result<Outcome> {
    current(session)?;
    match f(session)? {
        Some(l) => Ok(Outcome::said(l)),
        // a command that changed nothing is not a change, so do not pretend otherwise
        None => Ok(Outcome::said("No change")),
    }
}

/// Where a ground command works: `x y` when they are on the line, else under the selection.
///
/// The height does not have to be given: the brush finds its tile from x and y, and `flatten`
/// reads the height that is already there. So the point's z is only ever a placeholder.
fn ground_point(session: &Session, nums: &[&str]) -> Result<DVec3> {
    if nums.len() >= 2 {
        let usage = "x y must be numbers";
        let x: f64 = nums[0].parse().map_err(|_| anyhow::anyhow!(usage))?;
        let y: f64 = nums[1].parse().map_err(|_| anyhow::anyhow!(usage))?;
        return Ok(DVec3::new(x, y, 0.0));
    }
    Ok(session.selection().map(|s| s.position()).unwrap_or(DVec3::ZERO))
}

fn info(session: &Session) -> String {
    let d = session.doc();
    let writes = match d.destination() {
        Destination::Content(root) => format!("copies under {}", root.display()),
        Destination::InPlace => "the map's own files, with a .openomsi-bak snapshot".into(),
    };
    let saved = if d.is_dirty() {
        format!("changed - {} tile(s) to write", d.dirty_tiles().len())
    } else {
        "everything written".into()
    };
    [
        format!("map        {}", d.global().name),
        format!("config     {}", d.map_cfg().display()),
        format!("directory  {}", d.map_dir().display()),
        format!("tiles      {} ({} read)", d.tile_count(), d.loaded_tiles()),
        format!(
            "objects    {}{}",
            d.object_count(),
            if d.added_count() > 0 {
                format!(", {} placed this session", d.added_count())
            } else {
                String::new()
            }
        ),
        format!("writes     {writes}"),
        format!("state      {saved}"),
        format!("next id    {}", d.next_id()),
        format!("brush      {:.1} m", session.brush()),
    ]
    .join("\n")
}

/// How many objects a listing prints before it stops.
const LIMIT: usize = 40;

fn list(session: &Session, rest: &[&str]) -> Result<String> {
    let d = session.doc();
    let (tile, words): (Option<(i32, i32)>, Vec<&str>) = match (rest.first(), rest.get(1)) {
        (Some(x), Some(y)) if x.parse::<i32>().is_ok() && y.parse::<i32>().is_ok() => (Some((x.parse()?, y.parse()?)), rest[2..].to_vec()),
        _ => (None, rest.to_vec()),
    };
    let needle = words.join(" ").to_ascii_lowercase();
    let mut out = Vec::new();
    let mut total = 0usize;
    for (id, o) in d.objects() {
        if tile.is_some_and(|t| o.tile != t) {
            continue;
        }
        if !needle.is_empty() && !o.file.to_ascii_lowercase().contains(&needle) {
            continue;
        }
        total += 1;
        if out.len() >= LIMIT {
            continue;
        }
        let e = d.edit(id);
        let flag = if e.deleted { "  [taken away]" } else { "" };
        out.push(format!(
            "{:>7}  tile {:>4},{:<4} {:>9.2} {:>9.2} {:>7.2}  {:>6.1}\u{b0}  {}{flag}",
            id,
            o.tile.0,
            o.tile.1,
            o.pos.x + e.moved.x,
            o.pos.y + e.moved.y,
            o.pos.z + e.moved.z,
            o.heading + e.turned,
            o.file_name()
        ));
    }
    if total == 0 {
        return Ok("No object matches".into());
    }
    if total > out.len() {
        out.push(format!("... and {} more", total - out.len()));
    }
    out.push(format!("{total} object(s)"));
    Ok(out.join("\n"))
}

// ---- little parsers that say what they wanted --------------------------------------

fn need_f64(rest: &[&str], i: usize, usage: &str) -> Result<f64> {
    match rest.get(i) {
        Some(w) => w.parse().map_err(|_| anyhow::anyhow!("{usage}")),
        None => bail!("{usage}"),
    }
}

fn need_i64(rest: &[&str], i: usize, usage: &str) -> Result<i64> {
    match rest.get(i) {
        Some(w) => w.parse().map_err(|_| anyhow::anyhow!("{usage}")),
        None => bail!("{usage}"),
    }
}

/// The tile two numbers name, after the word that says what to do with it.
fn need_tile(rest: &[&str], usage: &str) -> Result<(i32, i32)> {
    Ok((need_i64(rest, 1, usage)? as i32, need_i64(rest, 2, usage)? as i32))
}

/// The `.sco` files the content folder holds, matching `filter` - what `new` can name, and
/// what the assets page lists.
///
/// Long lists are cut: an installation's `Sceneryobjects` holds thousands of files, and a
/// thousand lines in the console is not a list anybody reads.
fn assets(session: &Session, filter: &str) -> String {
    let root = session.doc().install_root();
    let all = omsi_editor_core::assets::scenery_objects(root);
    if all.is_empty() {
        return format!("No Sceneryobjects folder under {}", root.display());
    }
    let needle = filter.trim().to_ascii_lowercase();
    let shown: Vec<&String> = all
        .iter()
        .filter(|s| needle.is_empty() || s.to_ascii_lowercase().contains(&needle))
        .collect();
    if shown.is_empty() {
        return format!("None of the {} .sco files here matches that", all.len());
    }
    let mut out = shown
        .iter()
        .take(ASSETS_SHOWN)
        .map(|s| s.as_str())
        .collect::<Vec<_>>()
        .join("\n");
    if shown.len() > ASSETS_SHOWN {
        out.push_str(&format!("\n... and {} more", shown.len() - ASSETS_SHOWN));
    } else {
        out.push_str(&format!("\n({} of {})", shown.len(), all.len()));
    }
    out
}

/// How many of them the console prints before it says how many are left.
const ASSETS_SHOWN: usize = 200;

/// The map's tiles, in the order its own list gives them, with what each holds.
///
/// That order is the map's own numbering and not a sort: an entry point and a track file name
/// a tile by its place in this list, so this is the list a person has to see to make sense of
/// either of them.
fn tiles(session: &Session) -> String {
    let all = session.tiles();
    if all.is_empty() {
        return "This map lists no tile at all".into();
    }
    let lines: Vec<String> = all
        .iter()
        .enumerate()
        .map(|(i, tile)| format!("{i:>3}  {}", one_tile(session, *tile, "")))
        .collect();
    lines.join("\n")
}

/// One tile as a line: where it is, its file, and what it holds.
fn one_tile(session: &Session, tile: (i32, i32), note: &str) -> String {
    let fresh = session.doc().new_tiles().any(|t| t == tile);
    format!(
        "({}, {})  {}  {} object(s){}{}",
        tile.0,
        tile.1,
        tile_file_name(tile),
        session.objects_in(tile),
        if fresh { "  [added, not written yet]" } else { "" },
        if note.is_empty() { String::new() } else { format!("  {note}") }
    )
}

/// Three numbers, whether written `1 2 3` or `1,2,3`.
fn need_vec3(rest: &[&str], i: usize, usage: &str) -> Result<DVec3> {
    let joined = rest[i.min(rest.len())..].join(" ");
    let parts: Vec<&str> = if joined.contains(',') {
        joined.split(',').map(|s| s.trim()).collect()
    } else {
        joined.split_whitespace().collect()
    };
    if parts.len() < 3 {
        bail!("{usage}");
    }
    let v: Result<Vec<f64>> = parts[..3].iter().map(|p| p.parse::<f64>().map_err(|_| anyhow::anyhow!("{usage}"))).collect();
    let v = v?;
    Ok(DVec3::new(v[0], v[1], v[2]))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn three_numbers_are_read_with_or_without_commas() {
        assert_eq!(need_vec3(&["1", "2", "3"], 0, "v").unwrap(), DVec3::new(1.0, 2.0, 3.0));
        assert_eq!(need_vec3(&["1,2,3"], 0, "v").unwrap(), DVec3::new(1.0, 2.0, 3.0));
        assert_eq!(need_vec3(&["-1.5", "0", "2.25"], 0, "v").unwrap(), DVec3::new(-1.5, 0.0, 2.25));
        // an offset skips the words before the numbers
        assert_eq!(need_vec3(&["name.sco", "4", "5", "6"], 1, "v").unwrap(), DVec3::new(4.0, 5.0, 6.0));
        assert!(need_vec3(&["1", "2"], 0, "v").is_err());
        assert!(need_vec3(&[], 0, "v").is_err());
    }

    #[test]
    fn a_missing_number_says_what_was_wanted() {
        assert_eq!(need_f64(&[], 0, "turn <deg>").unwrap_err().to_string(), "turn <deg>");
        assert_eq!(need_f64(&["x"], 0, "turn <deg>").unwrap_err().to_string(), "turn <deg>");
        assert_eq!(need_i64(&["9"], 0, "sel <id>").unwrap().to_string(), "9");
    }

    #[test]
    fn the_help_lists_every_command_the_parser_knows() {
        for c in [
            "info", "ls", "sel", "where", "variants", "mv", "move", "turn", "face", "del", "undel", "reset", "place", "brush", "raise", "lower",
            "flatten", "flatten-to", "undo", "redo", "history", "save", "help", "quit", "assets", "new", "arm", "tiles", "tile add", "tile rm",
        ] {
            assert!(HELP.contains(c), "help does not mention {c}");
        }
    }
}
