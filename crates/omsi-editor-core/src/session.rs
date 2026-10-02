//! An editing session: the map, what has been done to it, what is selected, and the brush.
//!
//! This is the layer a front end talks to. `omsi-editor`'s terminal today and its window
//! tomorrow drive the same [`Session`] the same way - they differ only in how a change is
//! asked for. Keeping the selection, the brush size and the history here rather than in the
//! front end is what stops the two from drifting apart, which is exactly what went wrong with
//! the in-game editor's keystrokes touching the world directly.

use crate::command::{Command, History};
use crate::document::{file_name_of, folder_of, Destination, Document, NewObject, ObjectRef, SaveReport};
use crate::ground::{GroundAction, BRUSH_DEFAULT, BRUSH_MAX, BRUSH_MIN};
use crate::record::ObjectEdit;
use crate::EditError;
use glam::DVec3;

/// What the session is looking at, and what has been done to it.
#[derive(Clone, Debug)]
pub struct Selection {
    pub id: i64,
    /// The object as the map file has it.
    pub object: ObjectRef,
    /// What has been done to it since.
    pub edit: ObjectEdit,
    /// Whether this object was placed in this session rather than read from the map.
    pub placed: bool,
}

impl Selection {
    /// Where it stands: across and along as the map measures them, and its own height over the
    /// ground under it.
    ///
    /// The height is the odd one out and it is OMSI's doing: a record's z is a height *over*
    /// the terrain, which the game puts back under the object when it draws it, so it is not a
    /// height on the map. Everything that plots a place on the map - the crosshair, a ray, the
    /// camera - gives an absolute one, and the two are not interchangeable (see
    /// [`Session::move_selected_on_the_ground`]).
    pub fn position(&self) -> DVec3 {
        self.object.pos + self.edit.moved
    }

    /// Its heading now.
    pub fn heading(&self) -> f64 {
        self.object.heading + self.edit.turned
    }

    /// Just the `.sco`'s file name.
    pub fn name(&self) -> String {
        self.object.file_name()
    }
}

/// An open map, the history of what was done to it, and what is selected.
pub struct Session {
    doc: Document,
    history: History,
    selection: Option<i64>,
    brush: f64,
    /// A drag in progress: the object it is on, and what had been done to it when it began.
    /// See [`Session::begin_gesture`].
    dragging: Option<(i64, ObjectEdit)>,
}

impl Session {
    pub fn open(map_cfg: impl AsRef<std::path::Path>, destination: Destination) -> Result<Self, EditError> {
        Ok(Session {
            doc: Document::open(map_cfg, destination)?,
            history: History::new(),
            selection: None,
            brush: BRUSH_DEFAULT,
            dragging: None,
        })
    }

    pub fn open_in_place(map_cfg: impl AsRef<std::path::Path>) -> Result<Self, EditError> {
        Session::open(map_cfg, Destination::InPlace)
    }

    pub fn doc(&self) -> &Document {
        &self.doc
    }

    /// The document, for a front end that has to look something up itself.
    pub fn doc_mut(&mut self) -> &mut Document {
        &mut self.doc
    }

    pub fn history(&self) -> &History {
        &self.history
    }

    // ---- selection -------------------------------------------------------------------

    pub fn select(&mut self, id: Option<i64>) {
        self.selection = id.filter(|id| self.doc.object(*id).is_some() || self.doc.added_object(*id).is_some());
    }

    pub fn selected_id(&self) -> Option<i64> {
        self.selection
    }

    pub fn selection(&self) -> Option<Selection> {
        let id = self.selection?;
        let edit = self.doc.edit(id);
        if let Some(object) = self.doc.object(id) {
            return Some(Selection { id, object: object.clone(), edit, placed: false });
        }
        // something placed in this session: it has no record of its own yet, and where it is
        // is what its copy carries rather than an edit beside a record
        let a = self.doc.added_object(id)?;
        Some(Selection {
            id,
            object: ObjectRef { tile: a.tile, pos: a.base, heading: a.base_heading, file: a.sco.clone() },
            edit,
            placed: true,
        })
    }

    // ---- the brush -------------------------------------------------------------------

    pub fn brush(&self) -> f64 {
        self.brush
    }

    /// Set the brush's radius, clamped to what the brush allows.
    pub fn set_brush(&mut self, radius: f64) -> f64 {
        self.brush = clamp_brush(radius);
        self.brush
    }

    /// Grow (or shrink) the brush by a factor.
    pub fn scale_brush(&mut self, factor: f64) -> f64 {
        self.set_brush(self.brush * factor)
    }

    // ---- changes ---------------------------------------------------------------------

    /// Do a command, remembering how to take it back.
    pub fn do_(&mut self, cmd: Command) -> Result<Option<String>, EditError> {
        self.history.apply(&mut self.doc, cmd)
    }

    pub fn undo(&mut self) -> Result<Option<String>, EditError> {
        self.history.undo(&mut self.doc)
    }

    pub fn redo(&mut self) -> Result<Option<String>, EditError> {
        self.history.redo(&mut self.doc)
    }

    /// Write the map. A save moves the baseline; see [`Document::save`].
    pub fn save(&mut self) -> Result<SaveReport, EditError> {
        self.doc.save()
    }

    // ---- the gestures a front end makes, on the selection ----------------------------

    /// Move the selected object by this much.
    pub fn move_selected(&mut self, delta: DVec3) -> Result<Option<String>, EditError> {
        let id = self.selection.ok_or(EditError::Other("nothing is selected".into()))?;
        self.do_(Command::MoveObject { id, delta })
    }

    /// Move the selected object *to* a place, in the same terms [`Selection::position`] gives:
    /// across and along on the map, and z as a height over the ground.
    pub fn move_selected_to(&mut self, to: DVec3) -> Result<Option<String>, EditError> {
        let id = self.selection.ok_or(EditError::Other("nothing is selected".into()))?;
        let now = self.selection().map(|s| s.position()).ok_or(EditError::NoObject(id))?;
        self.do_(Command::MoveObject { id, delta: to - now })
    }

    /// Move the selected object to a place on the map, keeping the height it has over the
    /// ground - what clicking the map asks for.
    ///
    /// The place is a point on the ground (the crosshair's, or a ray's), and an object's own
    /// height is a height *over* the terrain rather than a height on the map. Read as one,
    /// dropping an object where the crosshair was lifted it by however high the map is there,
    /// which on a hill is a lamp post in the air. A height is changed a metre at a time -
    /// `mv 0 0 1`, or the up arrow of the gizmo - where the metres are metres.
    pub fn move_selected_on_the_ground(&mut self, at: DVec3) -> Result<Option<String>, EditError> {
        let id = self.selection.ok_or(EditError::Other("nothing is selected".into()))?;
        let now = self.selection().map(|s| s.position()).ok_or(EditError::NoObject(id))?;
        self.do_(Command::MoveObject { id, delta: DVec3::new(at.x - now.x, at.y - now.y, 0.0) })
    }

    /// Turn the selected object by this many degrees.
    pub fn turn_selected(&mut self, degrees: f64) -> Result<Option<String>, EditError> {
        let id = self.selection.ok_or(EditError::Other("nothing is selected".into()))?;
        self.do_(Command::TurnObject { id, delta: degrees })
    }

    /// Turn the selected object *to* a heading.
    pub fn turn_selected_to(&mut self, heading: f64) -> Result<Option<String>, EditError> {
        let id = self.selection.ok_or(EditError::Other("nothing is selected".into()))?;
        let now = self.selection().map(|s| s.heading()).ok_or(EditError::NoObject(id))?;
        self.do_(Command::TurnObject { id, delta: heading - now })
    }

    /// Set whether the selected object is in the map - the same command either way, so that
    /// deleting and putting back are each one undoable step.
    pub fn set_selected_deleted(&mut self, deleted: bool) -> Result<Option<String>, EditError> {
        let id = self.selection.ok_or(EditError::Other("nothing is selected".into()))?;
        let mut edit = self.doc.edit(id);
        edit.deleted = deleted;
        self.do_(Command::SetEdit { id, edit })
    }

    /// Put everything done to the selected object back.
    pub fn reset_selected(&mut self) -> Result<Option<String>, EditError> {
        let id = self.selection.ok_or(EditError::Other("nothing is selected".into()))?;
        self.do_(Command::SetEdit { id, edit: ObjectEdit::default() })
    }

    // ---- a drag ----------------------------------------------------------------------
    //
    // A gizmo dragged with the mouse says "put it here" sixty times a second, and sixty
    // steps in the history for one movement is not an undo - it is a punishment. The drag
    // therefore runs *outside* the history: it moves the object in the document, and the
    // single step is recorded when the mouse comes up (which is what `Command::Batch` is
    // for in a front end with several things in hand at once).

    /// Start a drag on the selected object. From here until [`Session::end_gesture`] the
    /// whole movement is one step in the history rather than one per frame.
    pub fn begin_gesture(&mut self) -> Option<i64> {
        let id = self.selection?;
        let before = self.doc.edit(id);
        self.dragging = Some((id, before));
        Some(id)
    }

    /// Whether a drag is in progress.
    pub fn gesture(&self) -> bool {
        self.dragging.is_some()
    }

    /// Show the object at `edit` - where the drag has taken it. Nothing goes in the history.
    pub fn drag_to(&mut self, edit: ObjectEdit) -> bool {
        let Some((id, _)) = self.dragging else { return false };
        self.doc.set_edit(id, edit).is_ok()
    }

    /// The drag is over: one step, from where the object stood to where it now is. A drag
    /// that ended where it started is not a step at all.
    pub fn end_gesture(&mut self) -> Result<Option<String>, EditError> {
        let Some((id, before)) = self.dragging.take() else { return Ok(None) };
        let after = self.doc.edit(id);
        if after == before {
            return Ok(None);
        }
        // the object goes back first: a step's inverse is worked out by *doing* the command,
        // and the history must see the object as it stood when the drag began
        self.doc.set_edit(id, before)?;
        self.do_(Command::SetEdit { id, edit: after })
    }

    /// Give the drag up: the object goes back where it stood.
    pub fn cancel_gesture(&mut self) {
        if let Some((id, before)) = self.dragging.take() {
            let _ = self.doc.set_edit(id, before);
        }
    }

    /// Place a copy of `template`.
    ///
    /// `file` names the `.sco` the copy gets, inside the template's own folder (the same file
    /// when it is `None`). `at` is where to put it; without one it goes two metres to the
    /// template's right, as the editor's key does. The copy becomes the selection.
    ///
    /// Copying something that was itself placed this session works too: the copy stands where
    /// that object stands, and its record is still written after the map object the two share.
    pub fn place_copy(&mut self, template: i64, file: Option<String>, at: Option<DVec3>, turn: f64) -> Result<Option<String>, EditError> {
        let map_object = self.doc.object(template).cloned();
        let (record_after, base, base_heading, tile, folder, default_name) = match &map_object {
            // the copy follows the object as it stands *now* - moved and turned this session
            // included. Copying where it was before the last move would put the copy somewhere
            // the person never pointed at, and a heading of the record's rather than the view's.
            Some(o) => {
                let e = self.doc.edit(template);
                (template, o.pos + e.moved, o.heading + e.turned, o.tile, folder_of(&o.file), o.file_name())
            }
            None => {
                let a = self.doc.added_object(template).cloned().ok_or(EditError::NoObject(template))?;
                let from = a
                    .template
                    .ok_or_else(|| EditError::Other(format!("object {template} was not placed from anything to copy")))?;
                (from, a.position(), a.heading(), a.tile, folder_of(&a.sco), file_name_of(&a.sco))
            }
        };
        let name = file.unwrap_or(default_name);
        let sco = format!("{folder}{name}");
        let template_record = self
            .doc
            .object_record(record_after)
            .ok_or_else(|| EditError::Other(format!("the record of object {record_after} could not be read")))?;

        // two metres to the right of how the template faces, when no place is given
        let h = base_heading.to_radians();
        let default_moved = DVec3::new(h.cos(), -h.sin(), 0.0) * 2.0;
        let id = self.doc.next_id();
        let new = NewObject {
            template: Some(record_after),
            tile,
            id,
            file: name,
            base,
            base_heading,
            moved: at.map(|p| p - base).unwrap_or(default_moved),
            turned: turn,
            deleted: false,
            sco,
            template_record,
        };
        let out = self.do_(Command::PlaceObject(new))?;
        self.selection = Some(id);
        Ok(out)
    }

    /// Give the selected placed object another `.sco` of its folder.
    pub fn set_selected_variant(&mut self, sco: String) -> Result<Option<String>, EditError> {
        let id = self.selection.ok_or(EditError::Other("nothing is selected".into()))?;
        self.do_(Command::SetVariant { id, sco })
    }

    /// The `.sco` files sitting beside the selected object - what a variant list offers.
    pub fn sibling_variants(&self) -> Vec<String> {
        let Some(s) = self.selection() else { return Vec::new() };
        let folder = s.object.folder().to_string_lossy().to_string();
        // through the resolver, so that a record's backslashes work on every platform
        let dir = omsi_cfg::resolve_path(self.doc.map_dir(), &folder);
        let Ok(entries) = std::fs::read_dir(&dir) else { return Vec::new() };
        let mut out: Vec<String> = entries
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.extension().is_some_and(|e| e.eq_ignore_ascii_case("sco")))
            .filter_map(|p| p.file_name().map(|n| n.to_string_lossy().to_string()))
            .collect();
        out.sort();
        out
    }

    // ---- the ground ------------------------------------------------------------------

    /// Raise (or lower, with a negative) the ground under `at`.
    pub fn raise_ground(&mut self, at: DVec3, metres: f64) -> Result<Option<String>, EditError> {
        self.do_(Command::ShapeGround { at, radius: self.brush, action: GroundAction::Raise(metres) })
    }

    /// Bring the ground under `at` to the height it already has there - a flatten.
    pub fn flatten_ground(&mut self, at: DVec3, to: Option<f64>) -> Result<Option<String>, EditError> {
        let target = match to {
            Some(h) => h,
            None => self.doc.ground_height(at.x, at.y).unwrap_or(at.z),
        };
        self.do_(Command::ShapeGround { at, radius: self.brush, action: GroundAction::Flatten(target) })
    }

    /// The height of the ground at a place.
    pub fn ground_height(&mut self, x: f64, y: f64) -> Option<f64> {
        self.doc.ground_height(x, y)
    }

    // ---- readouts --------------------------------------------------------------------

    /// The one-line summary a front end shows for the selection.
    pub fn selection_summary(&self) -> String {
        let Some(s) = self.selection() else {
            return "Nothing selected".into();
        };
        let e = s.edit;
        if e.deleted {
            return format!("{} {}: taken away", s.id, s.name());
        }
        if e.is_untouched() {
            return format!("{} {} at {}, {}, {}", s.id, s.name(), num(s.object.pos.x), num(s.object.pos.y), num(s.object.pos.z));
        }
        format!(
            "{} {}: moved by {}, {}, {} m, turned {:+.1}\u{b0}",
            s.id,
            s.name(),
            num(e.moved.x),
            num(e.moved.y),
            num(e.moved.z),
            e.turned
        )
    }

    /// The one-line summary of the whole session: what a title bar shows.
    pub fn title(&self) -> String {
        let mark = if self.doc.is_dirty() { "*" } else { "" };
        format!(
            "{}{mark} - {} tiles, {} objects{}",
            self.doc.global().name,
            self.doc.tile_count(),
            self.doc.object_count(),
            if self.doc.added_count() > 0 {
                format!(", {} placed", self.doc.added_count())
            } else {
                String::new()
            }
        )
    }
}

fn num(v: f64) -> String {
    crate::codec::num(v)
}

/// A brush radius, clamped to what the brush allows (m).
pub fn clamp_brush(radius: f64) -> f64 {
    if radius.is_nan() {
        BRUSH_DEFAULT
    } else {
        radius.clamp(BRUSH_MIN, BRUSH_MAX)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_brush_radius_is_clamped_to_what_the_brush_allows() {
        assert_eq!(clamp_brush(BRUSH_DEFAULT), BRUSH_DEFAULT);
        assert_eq!(clamp_brush(0.0), BRUSH_MIN);
        assert_eq!(clamp_brush(-5.0), BRUSH_MIN);
        assert_eq!(clamp_brush(1e9), BRUSH_MAX);
        assert_eq!(clamp_brush(f64::NAN), BRUSH_DEFAULT);
    }
}
