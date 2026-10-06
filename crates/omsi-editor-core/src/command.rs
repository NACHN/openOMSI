//! One change and its inverse.
//!
//! Everything the editor does to a map goes through a [`Command`]. Applying one returns the
//! command that puts it back, which is what makes undo work without any state being copied
//! wholesale: a move's inverse is the opposite move, a brush stroke's inverse is the handful
//! of height samples it touched.
//!
//! The in-game editor had no undo beyond "put this one object back where it started", because
//! a keystroke was applied straight to the world. Here the keystroke is a value first, which
//! is what lets the same change be taken back, redone, or logged.

use crate::document::{Document, HeldTile, NewObject, TileId};
use crate::ground::{GroundAction, GridRect};
use crate::record::ObjectEdit;
use crate::EditError;
use glam::DVec3;
use std::path::Path;

/// One change to a map.
#[derive(Clone, Debug, PartialEq)]
pub enum Command {
    /// Move an object by this much (m).
    MoveObject { id: i64, delta: DVec3 },
    /// Turn an object by this much (degrees clockwise).
    TurnObject { id: i64, delta: f64 },
    /// Set exactly what has been done to an object - the primitive the two above build on,
    /// and what an undo of either puts back.
    SetEdit { id: i64, edit: ObjectEdit },
    /// Place a new object.
    PlaceObject(NewObject),
    /// Take a placed object away again (the tile file loses its record).
    RemoveObject { id: i64 },
    /// Give a placed object another `.sco`.
    SetVariant { id: i64, sco: String },
    /// Shape the ground under `at` (world metres) within `radius`.
    ShapeGround { at: DVec3, radius: f64, action: GroundAction },
    /// Put these ground samples back as they were.
    RestoreTerrain { tile: TileId, rect: GridRect, before: Vec<f32> },
    /// Add a tile to the map's list of them, with its own empty file and its own flat ground.
    ///
    /// Appended to the list, so nothing that names a tile changes its number: adding a tile is
    /// safe where taking one away is not (see [`crate::tilemap`]).
    AddTile { tile: TileId },
    /// Take a tile out of the map's list, and everything that was in it with it. The tile's
    /// own file is left on disk - unlisted, not deleted.
    RemoveTile { tile: TileId },
    /// Put a tile back where it was, with everything it held. This is the inverse of a
    /// removal, and the only command that carries a [`HeldTile`] - an addition's inverse
    /// captures what the new tile holds when it is taken away again.
    RestoreTile { tile: TileId, at: usize, held: HeldTile },
    /// Several changes that were one gesture, undone together.
    Batch(Vec<Command>),
}

impl Command {
    /// What a status bar or an undo list calls this.
    pub fn label(&self) -> String {
        match self {
            Command::MoveObject { id, delta } => format!("Move object {id} ({} m)", fmt_delta(*delta)),
            Command::TurnObject { id, delta } => format!("Turn object {id} ({:+.1}\u{b0})", delta),
            Command::SetEdit { id, edit } if edit.deleted => format!("Take object {id} away"),
            Command::SetEdit { id, .. } => format!("Change object {id}"),
            Command::PlaceObject(o) => format!("Place {}", short(Path::new(&o.sco))),
            Command::RemoveObject { id } => format!("Take back object {id}"),
            Command::SetVariant { id, sco } => format!("Object {id} becomes {}", short(Path::new(sco))),
            Command::ShapeGround { action, radius, .. } => format!("{} (brush {} m)", action.describe(), crate::codec::num(*radius)),
            Command::RestoreTerrain { tile, .. } => format!("Restore the ground of tile ({}, {})", tile.0, tile.1),
            Command::AddTile { tile } => format!("Add the tile ({}, {})", tile.0, tile.1),
            Command::RemoveTile { tile } => format!("Take the tile ({}, {}) out of the map", tile.0, tile.1),
            Command::RestoreTile { tile, .. } => format!("Put the tile ({}, {}) back", tile.0, tile.1),
            Command::Batch(v) => match v.as_slice() {
                [one] => one.label(),
                [] => "Nothing".into(),
                many => format!("{} changes", many.len()),
            },
        }
    }

    /// Do it, and return the command that puts it back.
    pub fn apply(&self, doc: &mut Document) -> Result<Command, EditError> {
        match self {
            Command::MoveObject { id, delta } => {
                let before = doc.edit(*id);
                let mut e = before;
                e.moved += *delta;
                doc.set_edit(*id, e)?;
                Ok(Command::SetEdit { id: *id, edit: before })
            }
            Command::TurnObject { id, delta } => {
                let before = doc.edit(*id);
                let mut e = before;
                e.turned += *delta;
                doc.set_edit(*id, e)?;
                Ok(Command::SetEdit { id: *id, edit: before })
            }
            Command::SetEdit { id, edit } => {
                let before = doc.edit(*id);
                doc.set_edit(*id, *edit)?;
                Ok(Command::SetEdit { id: *id, edit: before })
            }
            Command::PlaceObject(obj) => {
                doc.place(obj.clone());
                Ok(Command::RemoveObject { id: obj.id })
            }
            Command::RemoveObject { id } => {
                let obj = doc.take_away(*id).ok_or(EditError::NoObject(*id))?;
                Ok(Command::PlaceObject(obj))
            }
            Command::SetVariant { id, sco } => {
                let before = doc.set_added_file(*id, sco.clone())?;
                Ok(Command::SetVariant { id: *id, sco: before })
            }
            Command::ShapeGround { at, radius, action } => match doc.shape_ground(*at, *radius, *action)? {
                Some((tile, rect, before)) => Ok(Command::RestoreTerrain { tile, rect, before }),
                // nothing was in reach: a change that was not one
                None => Ok(Command::Batch(Vec::new())),
            },
            Command::RestoreTerrain { tile, rect, before } => {
                let now = doc.capture_ground(*tile, rect)?;
                doc.restore_ground(*tile, rect, before)?;
                Ok(Command::RestoreTerrain { tile: *tile, rect: *rect, before: now })
            }
            // a new tile holds nothing, so taking it away again is the whole inverse
            Command::AddTile { tile } => {
                doc.add_tile(*tile)?;
                Ok(Command::RemoveTile { tile: *tile })
            }
            Command::RemoveTile { tile } => {
                let at = doc.tile_index(*tile).ok_or(EditError::NoTile(tile.0, tile.1))?;
                let held = doc.remove_tile(*tile)?;
                Ok(Command::RestoreTile { tile: *tile, at, held })
            }
            Command::RestoreTile { tile, at, held } => {
                doc.restore_tile(*tile, *at, held.clone())?;
                Ok(Command::RemoveTile { tile: *tile })
            }
            Command::Batch(cmds) => {
                let mut inverses = Vec::with_capacity(cmds.len());
                for c in cmds {
                    inverses.push(c.apply(doc)?);
                }
                // taken back in the opposite order to the one they were done in
                inverses.reverse();
                Ok(Command::Batch(inverses))
            }
        }
    }

    /// Whether this command did nothing (a brush stroke that found no ground).
    pub fn is_noop(&self) -> bool {
        matches!(self, Command::Batch(v) if v.is_empty())
    }
}

impl std::fmt::Display for Command {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.label())
    }
}

/// A step that has been done: the change, what takes it back, and the baseline it was done
/// against ([`Document::revision`]).
#[derive(Clone, Debug)]
struct Step {
    cmd: Command,
    inverse: Command,
    revision: u64,
}

/// What has been done this session, and what has been taken back.
///
/// A new change drops the redo stack: what was undone is only still reachable while nothing
/// has been done since, which is what every editor does.
///
/// A save drops the whole history. Saving moves the baseline the edits were measured against
/// (see [`Document::save`]), so a step from before it no longer means anything - taking it back
/// would not undo the change, it would apply a stale one. Rather than do that quietly, the
/// steps are dropped and [`History::undo`] reports that there is nothing to take back.
#[derive(Default)]
pub struct History {
    done: Vec<Step>,
    undone: Vec<Step>,
}

impl History {
    pub fn new() -> Self {
        History::default()
    }

    /// Do `cmd`, remembering how to take it back. Returns its label, or `None` when it turned
    /// out to be no change at all (and so is not in the history).
    pub fn apply(&mut self, doc: &mut Document, cmd: Command) -> Result<Option<String>, EditError> {
        let inverse = cmd.apply(doc)?;
        let label = cmd.label();
        if inverse.is_noop() {
            return Ok(None);
        }
        let revision = doc.revision();
        self.done.push(Step { cmd, inverse, revision });
        self.undone.clear();
        Ok(Some(label))
    }

    /// Take the last change back, unless a save has happened since.
    pub fn undo(&mut self, doc: &mut Document) -> Result<Option<String>, EditError> {
        if self.stale(doc) {
            return Ok(None);
        }
        let Some(step) = self.done.pop() else { return Ok(None) };
        let label = step.cmd.label();
        let _ = step.inverse.apply(doc)?;
        self.undone.push(step);
        Ok(Some(label))
    }

    /// Do the last taken-back change again, unless a save has happened since.
    pub fn redo(&mut self, doc: &mut Document) -> Result<Option<String>, EditError> {
        if self.stale(doc) {
            return Ok(None);
        }
        let Some(step) = self.undone.pop() else { return Ok(None) };
        let label = step.cmd.label();
        let inverse = step.cmd.apply(doc)?;
        let revision = doc.revision();
        self.done.push(Step { cmd: step.cmd, inverse, revision });
        Ok(Some(label))
    }

    /// Whether the top of the history was done against a baseline a save has since replaced.
    /// The whole history is dropped when it has: every step in it shares that baseline.
    fn stale(&mut self, doc: &Document) -> bool {
        let top = self.done.last().or_else(|| self.undone.last()).map(|s| s.revision);
        if top.is_some_and(|r| r != doc.revision()) {
            self.clear();
            return true;
        }
        false
    }

    pub fn can_undo(&self) -> bool {
        !self.done.is_empty()
    }

    pub fn can_redo(&self) -> bool {
        !self.undone.is_empty()
    }

    pub fn undo_label(&self) -> Option<String> {
        self.done.last().map(|s| s.cmd.label())
    }

    pub fn redo_label(&self) -> Option<String> {
        self.undone.last().map(|s| s.cmd.label())
    }

    pub fn depth(&self) -> usize {
        self.done.len()
    }

    /// The changes done, oldest first - what an undo-list panel shows.
    pub fn entries(&self) -> impl Iterator<Item = &Command> {
        self.done.iter().map(|s| &s.cmd)
    }

    pub fn clear(&mut self) {
        self.done.clear();
        self.undone.clear();
    }
}

/// Just the file name of a path, for a label.
fn short(p: &std::path::Path) -> String {
    p.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_else(|| p.to_string_lossy().to_string())
}

fn fmt_delta(d: DVec3) -> String {
    format!("{}, {}, {}", crate::codec::num(d.x), crate::codec::num(d.y), crate::codec::num(d.z))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// History needs no document for the parts that only read the stack.
    #[test]
    fn a_fresh_history_has_nothing_to_take_back() {
        let h = History::new();
        assert!(!h.can_undo() && !h.can_redo());
        assert_eq!(h.depth(), 0);
        assert_eq!(h.undo_label(), None);
        assert_eq!(h.redo_label(), None);
    }

    #[test]
    fn a_batch_is_labeled_by_what_it_holds() {
        let one = Command::MoveObject { id: 1, delta: DVec3::ZERO };
        assert_eq!(Command::Batch(vec![one.clone()]).label(), one.label());
        assert_eq!(Command::Batch(vec![]).label(), "Nothing");
        assert_eq!(Command::Batch(vec![one.clone(), one]).label(), "2 changes");
        assert!(Command::Batch(vec![]).is_noop());
    }

    #[test]
    fn an_object_label_names_the_object() {
        assert_eq!(Command::MoveObject { id: 17, delta: DVec3::new(1.0, 0.0, 0.0) }.label(), "Move object 17 (1, 0, 0 m)");
        assert_eq!(Command::TurnObject { id: 4, delta: 12.5 }.label(), "Turn object 4 (+12.5\u{b0})");
        assert_eq!(Command::SetEdit { id: 9, edit: ObjectEdit { deleted: true, ..Default::default() } }.label(), "Take object 9 away");
    }

    #[test]
    fn a_ground_label_says_what_it_did() {
        let c = Command::ShapeGround { at: DVec3::ZERO, radius: 6.0, action: GroundAction::Raise(0.25) };
        assert_eq!(c.label(), "Ground raised 0.25 m (brush 6 m)");
        let c = Command::ShapeGround { at: DVec3::ZERO, radius: 6.0, action: GroundAction::Raise(-0.25) };
        assert_eq!(c.label(), "Ground lowered 0.25 m (brush 6 m)");
    }
}
