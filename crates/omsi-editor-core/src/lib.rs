//! The map editor's headless core: what an editing session *is*, with no window, no GPU and
//! no simulation in it.
//!
//! openOMSI is three programs - the game (`openomsi`), the launcher (`openomsi-launcher`) and
//! the editor (`openomsi-editor`). This crate is the editor's bottom half: it can open a map,
//! edit it and write it back, and it can be tested from a terminal. The window, the camera,
//! the gizmos and the panels are the `omsi-editor` binary's business and live above it.
//!
//! * [`codec`]: a tile file's bytes as text and back, keeping the encoding it came in (OMSI's
//!   editor saves UTF-16LE, hand-made tiles are ASCII or Latin-1);
//! * [`record`]: an object record inside that text - moved, turned or taken away, with every
//!   other line, keyword and comment left exactly where it was;
//! * [`ground`]: the ground brush, shaping one tile's height field;
//! * [`document`]: the open map - its tiles, its object index, what has been changed, and how
//!   it is written back;
//! * [`newmap`]: a map where there was none - the one place here that writes a first version
//!   rather than a change to somebody else's;
//! * [`command`]: one change and its inverse, so every step can be taken back;
//! * [`session`]: the map, the history, the selection and the brush together - what a front
//!   end drives, whether that is a terminal or a window.
//!
//! # Why the files are edited as text
//!
//! A tile file holds records this project does not understand: mods add keywords, and people
//! hand-edit them. A parse/write round trip would drop those. So an edit here rewrites only
//! the lines it must and passes every other byte through untouched - comments, unknown
//! keywords, line endings and the original encoding included. `rewrite_tile` is the whole of
//! that rule, and the tests assert it byte for byte.
//!
//! # Where a save goes
//!
//! Never into the OMSI 2 installation. A changed tile and a changed `.map.terrain` are
//! written into the content folder as a copy, which the game reads before the installation
//! (see [`Document::open`]). A save that would land inside the original is refused.

pub mod codec;
pub mod command;
pub mod document;
pub mod ground;
pub mod newmap;
pub mod record;
pub mod session;

pub use codec::Encoding;
pub use command::{Command, History};
pub use document::{companion, tile_origin, Destination, Document, NewObject, ObjectRef, SaveReport, TileDoc, TileId};
pub use ground::{GroundAction, GridRect, BRUSH_DEFAULT, BRUSH_MAX, BRUSH_MIN};
pub use newmap::{clean_name, create as create_map, MadeMap};
pub use record::{add_copies, object_records, record_lines, rewrite_tile, NewRecord, ObjectEdit, ObjectRecord};
pub use session::{clamp_brush, Selection, Session};

/// What the editor refuses to do.
#[derive(Debug, thiserror::Error)]
pub enum EditError {
    #[error("{path}: {source}")]
    Cfg {
        path: std::path::PathBuf,
        #[source]
        source: omsi_cfg::CfgError,
    },
    #[error("{path}: {source}")]
    Io {
        path: std::path::PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("no object {0} in this map")]
    NoObject(i64),
    #[error("tile ({0}, {1}) is not in this map")]
    NoTile(i32, i32),
    #[error("the object {id} was not found in {path}")]
    NotInTile { id: i64, path: std::path::PathBuf },
    #[error("{0} lies in the original installation: not written")]
    RefusedWrite(std::path::PathBuf),
    #[error("{0}")]
    Other(String),
}

impl EditError {
    pub(crate) fn io(path: impl Into<std::path::PathBuf>, source: std::io::Error) -> Self {
        EditError::Io { path: path.into(), source }
    }
}
