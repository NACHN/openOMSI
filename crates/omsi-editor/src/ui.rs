//! The editor's own interface: the panels around the map.
//!
//! These are the launcher's widgets - the same buttons, fields, dropdowns and colours its
//! pages are drawn with, reached through `openomsi_game::host::ui` - so the editor looks like
//! the program the player already knows rather than like a console that grew a window.
//!
//! What the panels show is the *model* of the editing, which is the part the in-game editor
//! never made clear:
//!
//! - where a save writes (a copy under the content folder, or the map's own files),
//! - what is chosen, and what has been done to it,
//! - what the next click will change, which is the tool the rail shows,
//! - what is not written yet, and how to take it back,
//! - and a console, because the commands are still the sharpest tool there is - the terminal
//!   was only ever a poor place to keep them.
//!
//! Nothing here invents a control the core cannot do: the variant list appears only for an
//! object placed in this session (a `.sco` of a map object cannot be changed yet), and there
//! is no spline tool while splines cannot be edited.

use crate::repl;
use crate::start;
use crate::view::View;
use glam::{DVec3, Vec2};
use omsi_editor_core::{Destination, Document, Session, TileId, BRUSH_MAX, BRUSH_MIN};
use omsi_render::{Mark, Renderer};
use omsi_ui::paint::Align;
use omsi_ui::tr;
use omsi_ui::{Color, Draw, Gpu, Layer, Rect, Weight};
use openomsi_game::host::showroom::{Look, Showroom};
use openomsi_game::host::ui::{
    id_of, ButtonKind, Key, Ui, ACCENT, ACCENT_2, DANGER, EDGE, FIELD, HOVER, OK, PANEL, RAIL, SELECTED, TEXT, TEXT_DIM,
    TEXT_FAINT, TEXT_SOFT, WARN,
};
use winit::window::CursorIcon;

/// The bars' sizes, in logical pixels.
const TOP_H: f32 = 46.0;
const RAIL_W: f32 = 54.0;
/// The right-hand dock: four tabs, so it is wider than the one panel it used to be.
const SIDE_W: f32 = 296.0;
const STATUS_H: f32 = 28.0;
/// The hour the asset preview is lit at: the middle of a clear day (minutes past midnight).
const NOON: i32 = 12 * 60;
const CONSOLE_W: f32 = 430.0;
/// The tool options, beside the rail and over the map.
const OPTIONS_W: f32 = 210.0;

/// How high the view stands over the object the outline was told to go to (m). High enough
/// that what was chosen is in the frame with the map around it, low enough that it is not a
/// speck.
pub const JUMP_HEIGHT: f64 = 80.0;

/// How far one arrow key, or one `Nudge` press, moves a chosen object (m), and how far one
/// turn step turns it (degrees). The chip in the status bar cycles through them.
const STEPS: [f64; 5] = [0.25, 0.5, 1.0, 2.5, 5.0];
const TURN_STEPS: [f64; 5] = [5.0, 15.0, 30.0, 45.0, 90.0];

/// What a click on the map does.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Tool {
    /// Choose what is under the crosshair.
    Select,
    /// Drag what is chosen along the ground.
    Move,
    /// Drag left and right to turn what is chosen.
    Turn,
    /// Put a copy of what is chosen where the pointer meets the ground.
    Place,
    /// Take what is under the crosshair out of the map (`Restore` puts it back).
    Delete,
    /// Raise the ground under the crosshair.
    Raise,
    /// Flatten the ground under the crosshair to its own height.
    Flatten,
    /// The map's tiles: ringed over the map, and added or taken away by clicking one
    /// (`crate::tiles`).
    Tiles,
}

impl Tool {
    /// Every tool, in the order the rail shows them.
    pub const ALL: [Tool; 8] = [
        Tool::Select,
        Tool::Move,
        Tool::Turn,
        Tool::Place,
        Tool::Delete,
        Tool::Raise,
        Tool::Flatten,
        Tool::Tiles,
    ];

    fn icon(self) -> &'static str {
        match self {
            Tool::Select => "near_me",
            Tool::Move => "open_with",
            Tool::Turn => "autorenew",
            Tool::Place => "content_copy",
            Tool::Delete => "delete",
            Tool::Raise => "arrow_upward",
            Tool::Flatten => "remove",
            Tool::Tiles => "grid_view",
        }
    }

    fn name(self) -> &'static str {
        match self {
            Tool::Select => "Select",
            Tool::Move => "Move",
            Tool::Turn => "Turn",
            Tool::Place => "Place an object",
            Tool::Delete => "Take away",
            Tool::Raise => "Raise the ground",
            Tool::Flatten => "Flatten the ground",
            Tool::Tiles => "Edit the map's tiles",
        }
    }

    /// The key the window watches for it.
    pub fn key(self) -> char {
        match self {
            Tool::Select => 'V',
            Tool::Move => 'M',
            Tool::Turn => 'R',
            Tool::Place => 'C',
            Tool::Delete => 'X',
            Tool::Raise => 'G',
            Tool::Flatten => 'H',
            Tool::Tiles => 'T',
        }
    }

    /// The tool a key asks for - [`Tool::key`] read backwards, so that a tool's key is named
    /// in one place and not in two tables that can drift apart.
    pub fn from_key(c: char) -> Option<Tool> {
        Tool::ALL.iter().copied().find(|t| t.key().eq_ignore_ascii_case(&c))
    }

    /// One word, no spaces, untranslated: what a command line asks for (`--ui-tool move`).
    /// [`Tool::name`] is the other thing - a sentence for a tip, and translated.
    pub fn word(self) -> &'static str {
        match self {
            Tool::Select => "select",
            Tool::Move => "move",
            Tool::Turn => "turn",
            Tool::Place => "place",
            Tool::Delete => "delete",
            Tool::Raise => "raise",
            Tool::Flatten => "flatten",
            Tool::Tiles => "tiles",
        }
    }

    /// The tool a word asks for. `None` for a word that is not a tool, which the caller
    /// refuses out loud rather than quietly handing back the wrong one.
    pub fn named(word: &str) -> Option<Tool> {
        Tool::ALL.iter().copied().find(|t| t.word().eq_ignore_ascii_case(word))
    }

    /// Every word, for a usage message.
    pub fn words() -> impl Iterator<Item = &'static str> {
        Tool::ALL.iter().map(|t| t.word())
    }

    /// What the outline says a click would do to the object under the pointer with this tool in
    /// hand: a copy put down rings green, an object taken away rings red, and a tool that acts
    /// on what it is given leaves the neutral amber (see `Mark`).
    pub fn mark(self) -> Mark {
        match self {
            Tool::Place => Mark::Add,
            Tool::Delete => Mark::Remove,
            _ => Mark::Take,
        }
    }

    /// The colour this tool is shown in, wherever it is shown: its button in the rail, the
    /// line the status bar carries for it, and the ring it puts round what the pointer is on -
    /// so that a colour means one thing in the panels and in the picture alike.
    pub fn colour(self) -> Color {
        let c = self.mark().colour();
        Color::rgba(c[0], c[1], c[2], 1.0)
    }

    /// What the mouse does with this tool - the line the status bar carries.
    ///
    /// A click is one step to take back, and a drag is one step too: the gesture runs outside
    /// the history and the single step is recorded when the mouse comes up (see
    /// [`omsi_editor_core::Session::begin_gesture`]). The arrows do the fine work.
    fn hint(self) -> &'static str {
        match self {
            Tool::Select => "Left click chooses what the pointer is on",
            Tool::Move => "Left click puts it on the ground · drag an arrow to slide it",
            Tool::Turn => "Left click turns it to face the crosshair · drag the ring to turn it",
            Tool::Place => "Left click puts a copy where the pointer meets the ground",
            Tool::Delete => "Left click takes away what the pointer is on",
            Tool::Raise => "Left click raises the ground by the step · Shift lowers it",
            Tool::Flatten => "Left click levels the ground to where the pointer is",
            Tool::Tiles => "Left click adds the tile under the pointer · the map's own go back out of it",
        }
    }

    /// The line, given what the tool is holding.
    ///
    /// Only the place tool has two things it can do, and which one a click does is not visible
    /// from the button: a copy of the selection, or - with a `.sco` armed out of the content
    /// folder - a brand-new object of that file. So the line says which, because a person
    /// about to click the ground should not have to guess what will appear on it.
    pub fn hint_holding(self, armed: bool) -> &'static str {
        match (self, armed) {
            (Tool::Place, true) => "Left click puts a new object of the held .sco down",
            _ => self.hint(),
        }
    }

    /// The ground tools share the brush; the object tools share the selection.
    pub fn is_brush(self) -> bool {
        matches!(self, Tool::Raise | Tool::Flatten)
    }
}

/// What the right-hand dock is showing.
///
/// Five pages rather than the one panel there was, because editing asks five different
/// questions and they are asked of different things: what is chosen and what can be done to
/// it; which object, in a map of tens of thousands, to point at; which `.sco` out of the whole
/// content folder to put down; what has been done and how to take it back; and what a save
/// would write. None of them is invented - each is read from the session the terminal drives,
/// or - the assets page alone - from the folder the map is built out of.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Dock {
    /// The object that is chosen.
    Inspector,
    /// The map's objects, filtered - `sel 1687` without the console.
    Outline,
    /// The `.sco` files the content folder holds: what a new object is made of.
    Assets,
    /// What has been done this session, newest first.
    History,
    /// What a save would write.
    Unsaved,
}

impl Dock {
    /// Every page, in the order the tab strip shows them.
    const ALL: [Dock; 5] = [Dock::Inspector, Dock::Outline, Dock::Assets, Dock::History, Dock::Unsaved];

    fn name(self) -> &'static str {
        match self {
            Dock::Inspector => "Inspector",
            Dock::Outline => "Outline",
            Dock::Assets => "Assets",
            Dock::History => "History",
            Dock::Unsaved => "Unsaved",
        }
    }

    /// The page a name asks for - what `--ui-dock` sets, so that one of them can be drawn
    /// without clicking through to it.
    pub fn named(name: &str) -> Option<Dock> {
        Dock::ALL.iter().copied().find(|d| d.name().eq_ignore_ascii_case(name))
    }
}

/// The assets page's list: every `.sco` the content folder holds, and the part of it the
/// filter leaves.
///
/// Read once and kept, because it is the one list in these panels that is a whole folder tree
/// walked rather than a map indexed - and it is drawn sixty times a second.
struct AssetList {
    all: Vec<String>,
    shown: Vec<String>,
    /// The filter `shown` was read with, so that it is read again only when the filter moves.
    query: String,
}

/// One line of the outline: an object of the map, and what this session has done to it.
struct OutlineRow {
    id: i64,
    name: String,
    changed: bool,
    placed: bool,
    deleted: bool,
}

impl OutlineRow {
    /// The tag a row carries at its right, or none: what has happened to it, which is the one
    /// thing an id and a file name cannot say.
    fn badge(&self) -> Option<(&'static str, bool)> {
        // (the label, whether it is a warning)
        if self.deleted {
            Some(("taken away", true))
        } else if self.placed {
            Some(("placed", false))
        } else if self.changed {
            Some(("changed", false))
        } else {
            None
        }
    }

    /// Whether the filter leaves it.
    fn wanted(&self, query: &str, only_changed: bool) -> bool {
        if only_changed && !(self.changed || self.placed || self.deleted) {
            return false;
        }
        query.is_empty() || self.id.to_string().contains(query) || self.name.to_lowercase().contains(query)
    }
}

/// What the outline's list was read from.
///
/// A map holds tens of thousands of objects and the list cannot be walked once a frame, so it
/// is kept until one of these moves. The two sums are what catch a change no count would: an
/// object put back and another touched instead leaves the number of them the same.
#[derive(Clone, PartialEq)]
struct OutlineKey {
    query: String,
    only_changed: bool,
    revision: u64,
    objects: usize,
    /// Every id this session has touched, added up.
    touched: i64,
    /// Every id it has placed, added up.
    placed: i64,
}

/// Read the outline's rows: every object of the map, and the copies placed this session, with
/// the ones the filter leaves.
///
/// A copy taken away again is not here (nothing of it is in the map), and an object of the map
/// that was taken away still is - putting it back is what selecting it is for.
fn outline_rows(doc: &Document, query: &str, only_changed: bool) -> Vec<OutlineRow> {
    let q = query.trim().to_lowercase();
    let mut out: Vec<OutlineRow> = Vec::new();
    for (id, o) in doc.objects() {
        let e = doc.edit(id);
        let row = OutlineRow {
            id,
            name: o.file_name(),
            changed: !e.is_untouched() && !e.deleted,
            placed: false,
            deleted: e.deleted,
        };
        if row.wanted(&q, only_changed) {
            out.push(row);
        }
    }
    for a in doc.added() {
        let row = OutlineRow {
            id: a.id,
            name: omsi_editor_core::document::file_name_of(&a.sco),
            changed: false,
            placed: true,
            deleted: false,
        };
        if row.wanted(&q, only_changed) {
            out.push(row);
        }
    }
    out.sort_by_key(|r| r.id);
    out
}

/// What the panels show besides the session: numbers that belong to the view.
pub struct Info {
    pub fps: f32,
    pub loaded_tiles: usize,
    /// Where the middle of the view meets the ground, this frame.
    pub aim: Option<DVec3>,
    /// The gizmo handle the pointer is on, or the one a drag is holding: named along the
    /// status bar, because a handle that lights up under the pointer says it is there and
    /// nothing more - and a drag in hand is worth knowing about.
    pub holding: Option<String>,
}

/// One frame's worth of vertices, ready to be drawn over the map.
pub struct Frame {
    pub layers: Vec<Layer>,
    pub draws: Vec<Draw>,
}

/// A map the editor could open, as the start page shows it.
///
/// `from_content` is the answer to the question that decides what a save will touch: a map
/// found under the game's content folder is a mod's, and the one under the installation is
/// the original. It is the whole reason the list shows it (see `omsi_cfg::resolve_path`).
pub struct MapEntry {
    pub name: String,
    pub cfg: std::path::PathBuf,
    pub from_content: bool,
}

/// A vehicle the first page lists; see `crate::start::VehicleEntry`.
pub use crate::start::VehicleEntry;

/// What the start page says about the run, besides the maps: where they come from and where
/// a save would go. Both are read from the same places the terminal banner reads them.
pub struct Start<'a> {
    pub root: &'a std::path::Path,
    pub content: Option<&'a std::path::Path>,
    pub destination: &'a Destination,
}

/// The editor's panels, and the little state a click needs between frames.
pub struct Panels {
    ui: Ui,
    gpu: Option<Gpu>,
    /// The tool a click on the map uses.
    pub tool: Tool,
    /// Which step and turn step the arrows use.
    step: usize,
    turn_step: usize,
    /// Which page of the right-hand dock is open.
    dock: Dock,
    /// Whether the tool options are shown beside the rail. They hold the brush's size and the
    /// two steps, which used to sit in the status bar where they could only be read.
    options: bool,
    /// The outline's filter, and whether it shows only what this session touched.
    outline_query: String,
    outline_only_changed: bool,
    /// The assets page: the `.sco` files the content folder holds, and its filter. `None` until
    /// the page is first drawn - the walk is not something to do on the way to a map.
    assets: Option<AssetList>,
    asset_query: String,
    /// The launcher's showroom, drawing the asset the place tool is holding: the same picture
    /// of a model its Drive page shows, on the same floor under the same light, orbited the same
    /// way (`openomsi_game::host::showroom`). Nothing is loaded until there is something armed.
    showroom: Showroom,
    /// That picture as the toolkit sees it: a texture in its own GPU table, and the generation
    /// it was bound at - a picture made again at another size needs the view bound again.
    ///
    /// The card draws what was bound at the end of the frame before, which is the one-frame
    /// behind the showroom keeps for the launcher's own card as well.
    preview_tex: Option<usize>,
    preview_gen: u64,
    /// Where the picture was drawn this frame, and where the pointer was when it took hold of
    /// it. The panels have the pointer and the buttons, so the orbit lives here and not in the
    /// window.
    preview_rect: Option<Rect>,
    preview_drag: Option<Vec2>,
    /// Whether the outline shows the map's tiles rather than its objects - the two halves of
    /// the same question (see [`Panels::outline_tab`]).
    outline_tiles: bool,
    /// The rows the outline is showing, and the state they were read in.
    outline: Vec<OutlineRow>,
    outline_key: Option<OutlineKey>,
    /// Where the outline asked the view to go: the object it chose. The panels cannot move a
    /// camera - the view is the window's - so the window reads this and stands the view over
    /// the place (`window::Editor`).
    pub go_to: Option<DVec3>,
    /// How many steps the history tab was asked to take back, or to do again. Applied at the
    /// end of the tab, once the list that was clicked is no longer borrowed.
    history_back: Option<usize>,
    history_forward: Option<usize>,
    /// The console: folded away to see the map, or open as a list of what has happened.
    pub open: bool,
    /// What is being typed at the console prompt.
    command: String,
    /// What the console has said, oldest first.
    log: Vec<String>,
    /// The `.sco` files beside the chosen object, and which object they were read for - a
    /// directory listing every frame would be a file system call every frame.
    variants: Vec<String>,
    variants_for: Option<i64>,
    /// Set by the write chip when the other destination is asked for; the window reopens the
    /// session (only when nothing is unsaved).
    pub reopen: Option<Destination>,
    /// Set by [`Panels::build`]: the left button went down this frame and no panel took it,
    /// so it was meant for the map - what the window acts on after the frame is drawn.
    pub clicked_map: bool,
    /// Set by anything a panel did that may have changed the map's own list of tiles, or
    /// written one: the tiles tab's two buttons, the console, a save, an undo.
    ///
    /// A world streams the tile list it took when the map was opened, so a tile added to the
    /// list was invisible until the map was opened again - which is a reload the editor can do
    /// itself. The window reads this and hands the list over ([`crate::view::View::adopt_tiles`]).
    /// It is set generously rather than worked out
    /// precisely: what the window does with it is compare two lists, and it is the tile list
    /// in step with the map that matters, not the tidiness of who asked.
    pub tiles_changed: bool,
    /// Set when `quit` was typed at the console: the window is the program's way out now, so
    /// it reads this and closes (`window::Editor::about_to_wait`).
    pub quit: bool,

    /// The first page: which of the three things the rail offers is open, and what is
    /// highlighted in the list that page shows (`crate::start` draws it).
    page: start::State,
    /// Every map the first page offers. The window shows this page until a session is open,
    /// so a double click never lands in a terminal asking which map to edit.
    pub maps: Vec<MapEntry>,
    /// The vehicles under `Vehicles` in the two folders - what the vehicle page lists.
    pub vehicles: Vec<start::VehicleEntry>,
    /// A map the first page was asked to open. The window reads it, opens the session on it
    /// and draws the editing panels from then on.
    pub open_map: Option<std::path::PathBuf>,
    /// A new map the first page asked for, by the name that was typed: the window writes it
    /// and opens it (`Window::make_a_map`).
    pub new_map: Option<String>,
    /// Set by the top bar's way back: the window shows the page that lists the maps again,
    /// keeping the session it has (`Window::back_to_the_maps`).
    pub back: bool,
}

impl Default for Panels {
    fn default() -> Self {
        Self::new()
    }
}

impl Panels {
    pub fn new() -> Self {
        Panels {
            ui: Ui::new(),
            gpu: None,
            tool: Tool::Select,
            step: 2,
            turn_step: 1,
            dock: Dock::Inspector,
            options: true,
            outline_query: String::new(),
            outline_only_changed: false,
            assets: None,
            asset_query: String::new(),
            showroom: Showroom::new(),
            preview_tex: None,
            preview_gen: 0,
            preview_rect: None,
            preview_drag: None,
            outline_tiles: false,
            outline: Vec::new(),
            outline_key: None,
            go_to: None,
            history_back: None,
            history_forward: None,
            open: true,
            command: String::new(),
            log: vec![
                "Commands work here as they did in the terminal: help lists them.".to_string(),
                "The map is flown with W A S D, Q and E, the right button turns the view.".to_string(),
                "`maps` goes back to the list, to open another one.".to_string(),
            ],
            variants: Vec::new(),
            variants_for: None,
            reopen: None,
            clicked_map: false,
            tiles_changed: false,
            quit: false,
            page: start::State::default(),
            maps: Vec::new(),
            vehicles: Vec::new(),
            open_map: None,
            new_map: None,
            back: false,
        }
    }

    /// The maps the first page offers. The map editor's page is the one it opens on, so the
    /// highlight starts at the top of the list.
    pub fn set_maps(&mut self, maps: Vec<MapEntry>) {
        self.maps = maps;
        self.page.map_choice = 0;
    }

    /// The vehicles the vehicle page lists.
    pub fn set_vehicles(&mut self, vehicles: Vec<start::VehicleEntry>) {
        self.vehicles = vehicles;
        self.page.vehicle_choice = 0;
    }

    /// Set by the Maps page's "New map": the name to make a map by. The window writes the map
    /// and opens it, or says why it could not be made with [`Panels::new_map_failed`].
    pub fn take_new_map(&mut self) -> Option<String> {
        self.new_map.take()
    }

    /// A map could not be made. It is said under the field that named it, because that is
    /// where the name that failed is - and the name is still there to be changed.
    pub fn new_map_failed(&mut self, why: String) {
        self.page.new_map_error = Some(why);
    }

    /// Enter in the first page's name field: what the "New map" button beside it does.
    pub fn make_the_new_map(&mut self) {
        self.page.new_map_error = None;
        self.new_map = Some(self.page.new_map_name.clone());
    }

    /// The top bar's way back was clicked: show the page that lists the maps.
    pub fn take_back(&mut self) -> bool {
        std::mem::take(&mut self.back)
    }

    /// Open one of the first page's three sections, as a click on the rail would - what
    /// `--ui-section` sets, so that a page can be looked at without clicking through to it.
    pub fn show_section(&mut self, section: start::Section) {
        self.page.section = section;
    }

    /// Show one page of the right-hand dock, as a click on its tab would - what `--ui-dock`
    /// sets, so that a page can be looked at without clicking through to it.
    pub fn show_dock(&mut self, dock: Dock) {
        self.dock = dock;
    }

    /// Show the outline's other half, the map's tiles rather than its objects - what
    /// `--ui-tiles` sets.
    pub fn show_tiles(&mut self) {
        self.outline_tiles = true;
    }

    /// Move the highlight up or down the list the open page is showing - the arrow keys on
    /// the first page.
    pub fn choose(&mut self, step: i32) {
        let (len, choice) = match self.page.section {
            // the test track is not a list to walk
            start::Section::Testing => return,
            start::Section::Vehicles => (self.vehicles.len(), &mut self.page.vehicle_choice),
            start::Section::Maps => (self.maps.len(), &mut self.page.map_choice),
        };
        if len == 0 {
            return;
        }
        *choice = (*choice as i32 + step).rem_euclid(len as i32) as usize;
    }

    /// Open the highlighted map: what Enter does on the first page, and what a second click
    /// on an already-highlighted row does.
    pub fn open_chosen(&mut self) {
        if let Some(m) = self.maps.get(self.page.map_choice) {
            self.open_map = Some(m.cfg.clone());
        }
    }

    /// Add a line to the console.
    pub fn say(&mut self, line: impl Into<String>) {
        for l in line.into().lines() {
            if !l.trim().is_empty() {
                self.log.push(l.to_string());
            }
        }
        // (a session's listing can be thousands of lines; the panel shows the tail)
        let keep = 400;
        if self.log.len() > keep {
            self.log.drain(..self.log.len() - keep);
        }
    }

    /// Whether a text field has the keys - the window must not fly the camera meanwhile.
    pub fn typing(&self) -> bool {
        self.ui.focus.is_some()
    }

    /// Whether a panel is under the pointer: the map takes neither click nor wheel.
    pub fn over_ui(&self) -> bool {
        self.ui.over_ui
    }

    /// What the pointer should look like over the panels (a hand over a button, an I-beam in
    /// a field), or `None` when it is over the map.
    pub fn cursor(&self) -> Option<winit::window::CursorIcon> {
        self.ui.over_ui.then_some(self.ui.cursor)
    }

    /// The step the arrows and the nudge buttons use (m), and the turn step (degrees).
    pub fn step(&self) -> f64 {
        STEPS[self.step.min(STEPS.len() - 1)]
    }

    pub fn turn_step(&self) -> f64 {
        TURN_STEPS[self.turn_step.min(TURN_STEPS.len() - 1)]
    }

    /// The keyboard and mouse events go in before the frame is drawn; this is where the window
    /// hands them over.
    pub fn input(&mut self) -> &mut openomsi_game::host::ui::Input {
        &mut self.ui.input
    }

    /// The wheel over the map with a ground tool in hand: it is the brush that grows, not the
    /// camera's speed - a ground edit is aimed, and its size is the thing being chosen.
    pub fn brush_by(&mut self, session: &mut Session, factor: f64) {
        session.scale_brush(factor);
    }

    /// What was read for the chosen object's variant list is no longer right - the selection
    /// has moved on, or what is chosen has changed underneath it.
    pub fn selection_changed(&mut self) {
        self.variants_for = None;
    }

    /// Lay the panels out and upload them; the caller draws the result over the map with
    /// [`Panels::render`]. `size` is the window in physical pixels, `scale` its DPI factor.
    pub fn build(
        &mut self,
        session: &mut Session,
        view: &View,
        info: &Info,
        renderer: &mut Renderer,
        size: (u32, u32),
        scale: f32,
        dt: f32,
    ) -> Frame {
        let w = size.0 as f32 / scale;
        let h = size.1 as f32 / scale;
        self.ui.begin(Vec2::new(w, h), scale, dt);
        // the asset preview stepped: the showroom reads a `.sco` on its own thread and draws a
        // picture when the read is done, so it has to be given every frame - but it is told what
        // to read only by the assets page, and loads nothing while nothing is armed
        self.showroom.update(renderer, dt);
        self.preview_rect = None;
        self.layout(session, view, info, w, h);
        // a click the panels did not take is the map's. Read here and not by the window,
        // because only now are the panels laid out (`over_ui`), and because `finish` uses the
        // frame's input up - a press that started on a button must not also land on the map.
        self.clicked_map = self.ui.input.pressed && !self.ui.input.right_down && !self.ui.over_ui && self.ui.focus.is_none();
        let frame = self.upload(renderer);
        // the showroom's picture, once the card that shows it has been laid out and the
        // toolkit's GPU table exists to put it in - see [`Panels::preview_picture`]
        self.preview_picture(renderer, scale);
        frame
    }

    /// The start page: the maps there are to open, drawn over nothing at all.
    ///
    /// It is the launcher's first page in spirit - the one thing the editor cannot guess is
    /// which map someone means - so it is a page of the same kind: a card in the middle, a
    /// list, and what is known about the run along the bottom of it.
    pub fn build_start(&mut self, start: &Start, renderer: &Renderer, size: (u32, u32), scale: f32, dt: f32) -> Frame {
        let w = size.0 as f32 / scale;
        let h = size.1 as f32 / scale;
        self.ui.begin(Vec2::new(w, h), scale, dt);
        // the fields are taken apart so that the toolkit and the lists it draws can be
        // borrowed at the same time - the page reads them both
        let mut open = None;
        let mut made = None;
        let mut quit = false;
        {
            let Panels { ui, page, maps, vehicles, .. } = self;
            let data = crate::start::Data {
                root: start.root,
                content: start.content,
                destination: start.destination,
                maps,
                vehicles,
            };
            match crate::start::draw(ui, page, &data, w, h) {
                crate::start::Action::None => {}
                crate::start::Action::Open(i) => open = maps.get(i).map(|m| m.cfg.clone()),
                crate::start::Action::NewMap(name) => made = Some(name),
                crate::start::Action::Quit => quit = true,
            }
        }
        if let Some(cfg) = open {
            self.open_map = Some(cfg);
        }
        if let Some(name) = made {
            self.new_map = Some(name);
        }
        if quit {
            self.quit = true;
        }
        // there is no map behind this page, so there is no map click either
        self.clicked_map = false;
        self.upload(renderer)
    }

    /// Turn the frame the widgets drew into vertices on the card: the last step either page
    /// takes, and the only one that needs the renderer.
    fn upload(&mut self, renderer: &Renderer) -> Frame {
        let (layers, verts, ranges) = self.ui.finish();
        let atlas = self.ui.atlas.size;
        // One sample, deliberately. The panels are drawn *over* the map into the same target,
        // and a resolved MSAA pass writes every pixel of that target - the map with it - so
        // `clear: None` would put the map out rather than leave it standing. Nothing is lost
        // by it: a rounded corner is antialiased in the shader (see `rounded_mask`), which
        // is the only place the launcher's four samples were doing any work.
        let gpu = self.gpu.get_or_insert_with(|| Gpu::new(&renderer.device, renderer.format(), 1, atlas));
        gpu.upload(&renderer.device, &renderer.queue, 0, &verts);
        gpu.upload_atlas(&renderer.queue, &mut self.ui.atlas);
        let draws = ranges
            .iter()
            .enumerate()
            .map(|(k, (r, tex))| Draw { buffer: 0, range: r.clone(), layer: k, texture: *tex })
            .collect();
        Frame { layers, draws }
    }

    /// Draw `frame` over whatever is already in `target` - the map, drawn first.
    pub fn render(
        &mut self,
        frame: &Frame,
        renderer: &Renderer,
        encoder: &mut wgpu::CommandEncoder,
        target: &wgpu::TextureView,
        size: (u32, u32),
    ) {
        if let Some(gpu) = self.gpu.as_mut() {
            // no clear: the map stays under the panels
            gpu.render(&renderer.device, &renderer.queue, encoder, target, size, None, &frame.layers, &frame.draws);
        }
    }

    // ---- the panels ---------------------------------------------------------------------

    fn layout(&mut self, session: &mut Session, view: &View, info: &Info, w: f32, h: f32) {
        let top = Rect::new(0.0, 0.0, w, TOP_H);
        let rail = Rect::new(0.0, TOP_H, RAIL_W, h - TOP_H - STATUS_H);
        let side = Rect::new(w - SIDE_W, TOP_H, SIDE_W, h - TOP_H - STATUS_H);
        let status = Rect::new(0.0, h - STATUS_H, w, STATUS_H);
        self.top_bar(session, top);
        self.rail(rail);
        // the tool options, beside the rail and over the map: what the tool in hand can be
        // told to do, next to the button that chose it. Its height is the blocks it draws.
        if self.options {
            let brush = if self.tool.is_brush() { 18.0 + 30.0 } else { 0.0 };
            let place = if self.tool == Tool::Place { 18.0 + 32.0 } else { 0.0 };
            // the tile tool's own block, and taller than it is when there is nothing waiting
            let waiting = if self.tool == Tool::Tiles { self.pending_tiles_height(session, OPTIONS_W - 24.0) } else { 0.0 };
            let high = 10.0 + 22.0 + waiting + brush + place + 2.0 * (18.0 + 32.0) + 12.0;
            self.options_flyout(session, Rect::new(RAIL_W + 10.0, TOP_H + 12.0, OPTIONS_W, high));
        }
        self.dock(session, info, side);
        let armed = session.armed_asset().is_some();
        self.status_bar(view, info, armed, status);
        let h = if self.open { 168.0 } else { 30.0 };
        let console = Rect::new(RAIL_W + 14.0, status.y - 14.0 - h, CONSOLE_W, h);
        self.console(session, console);
    }

    /// The bar across the top: which map, where a save writes, what is not written, and the
    /// undo / redo / save that act on it.
    fn top_bar(&mut self, session: &mut Session, r: Rect) {
        self.ui.p().rect(r, RAIL);
        self.ui.p().rect(Rect::new(r.x, r.bottom() - 1.0, r.w, 1.0), EDGE);
        self.ui.solid(r);

        let name = session.doc().global().name.clone();

        // the way back to the page that asks which map. An editor whose only way off a map is
        // closing the window is a trap, and the window is where the list is - a terminal in
        // front of it stopped being the way in.
        let mid = r.center().y;
        if self.ui.icon_button("top-maps", Vec2::new(28.0, mid), 16.0, "arrow_back", &tr("Back to the maps")) {
            self.back = true;
        }

        self.ui.text_in(&name, Rect::new(52.0, r.y, 320.0, r.h), 14.0, Weight::Bold, TEXT, Align::Left);
        let named = self.ui.width(&name, 14.0, Weight::Bold);
        let file = session.doc().map_cfg().to_string_lossy().to_string();
        self.ui
            .text_in(&file, Rect::new(52.0 + named + 12.0, r.y, 560.0, r.h), 11.5, Weight::Regular, TEXT_FAINT, Align::Left);

        // from the right: save, redo, undo, then what a save would do and what is waiting
        let mut x = r.right() - 16.0;
        // What a save would write is two questions and not one: which tiles have changed, and
        // whether the map's own list of them has - a tile added or taken away changes no tile
        // at all, so a map whose only change is that one has nothing in the first count.
        let dirty = session.doc().dirty_tiles().len();
        let listed = session.doc().is_listed_dirty();
        let can_save = session.doc().is_dirty();
        x -= 104.0;
        let kind = if can_save { ButtonKind::Primary } else { ButtonKind::Ghost };
        if self.ui.button("top-save", Rect::new(x, mid - 15.0, 104.0, 30.0), "Save", Some("save"), kind) && can_save {
            self.save(session);
        }
        x -= 40.0;
        let can_redo = session.history().can_redo();
        let redo_tip = session.history().redo_label().unwrap_or_else(|| "Nothing to do again".into());
        if self.ui.icon_button("top-redo", Vec2::new(x + 14.0, mid), 15.0, "turn_right", &redo_tip) && can_redo {
            self.undo(session, false);
        }
        x -= 34.0;
        let can_undo = session.history().can_undo();
        let undo_tip = session.history().undo_label().unwrap_or_else(|| "Nothing to take back".into());
        if self.ui.icon_button("top-undo", Vec2::new(x + 14.0, mid), 15.0, "turn_left", &undo_tip) && can_undo {
            self.undo(session, true);
        }
        x -= 14.0;

        // what is waiting to be written. The words are a translation unit of their own, so a
        // language without a plural - most of them - can say it without the "(s)"
        let (label, colour) = if !can_save {
            ("No changes yet".to_string(), OK)
        } else if listed && dirty == 0 {
            (tr("the tile list has changed").to_string(), WARN)
        } else {
            (format!("{dirty} {}", tr("tiles not written")), WARN)
        };
        x -= chip(&mut self.ui, Vec2::new(0.0, mid - 13.0), &label, colour, x).0;
        x -= 8.0;

        // where a save writes - the one thing about editing a map that has to be said out loud.
        // The tip is built a sentence at a time: a whole sentence is what a translation table
        // holds, so the path goes on a line of its own between them
        let (short, tip) = match session.doc().destination() {
            Destination::Content(p) => (
                "Writes copies".to_string(),
                format!(
                    "{}\n{}\n{}\n{}",
                    tr("A changed tile is written as a copy under"),
                    p.display(),
                    tr("The game reads that copy before the original map, which is never touched."),
                    tr("Click to write the map's own files instead.")
                ),
            ),
            Destination::InPlace => (
                "Writes the map".to_string(),
                format!(
                    "{}\n{}\n{}",
                    tr("A changed tile is written over the map's own file."),
                    tr("A .openomsi-bak snapshot is kept beside each file it changes."),
                    tr("Click to write copies instead.")
                ),
            ),
        };
        let (w_chip, clicked) = chip(&mut self.ui, Vec2::new(0.0, mid - 13.0), &short, ACCENT_2, x);
        let rect = Rect::new(x - w_chip, mid - 13.0, w_chip, 26.0);
        self.ui.tooltip(rect, &tip);
        if clicked {
            if dirty == 0 {
                self.reopen = Some(match session.doc().destination() {
                    Destination::Content(_) => Destination::InPlace,
                    Destination::InPlace => Destination::Content(default_content()),
                });
            } else {
                self.say("Save or take the changes back before writing somewhere else.");
            }
        }
    }

    /// The tools, down the left edge.
    fn rail(&mut self, r: Rect) {
        self.ui.p().rect(r, RAIL);
        self.ui.p().rect(Rect::new(r.right() - 1.0, r.y, 1.0, r.h), EDGE);
        self.ui.solid(r);
        let mut y = r.y + 12.0;
        for (i, tool) in Tool::ALL.iter().enumerate() {
            // the object tools above, the ground tools below
            if tool.is_brush() && i > 0 && !Tool::ALL[i - 1].is_brush() {
                self.ui.p().rect(Rect::new(r.x + 14.0, y + 2.0, r.w - 28.0, 1.0), EDGE);
                y += 13.0;
            }
            let slot = Rect::new(r.x + 11.0, y, 32.0, 32.0);
            // the tool in hand carries the colour its outline rings an object in, so that what
            // a colour means in the picture is what it means on the button
            let in_hand = self.tool == *tool;
            if in_hand {
                self.ui.p().rounded(slot, 8.0, SELECTED);
                self.ui.p().rounded(Rect::new(slot.x, slot.y + 7.0, 2.0, slot.h - 14.0), 1.0, tool.colour());
            }
            // the name is translated first: the tip goes through the drawing call as one
            // string, so `format!`ing it here would put "Move (M)" in the table instead
            let tip = format!("{} ({})", tr(tool.name()), tool.key());
            let tint = in_hand.then(|| tool.colour());
            if self.ui.icon_button_in(&format!("tool{}", tool.key()), slot.center(), 16.0, tool.icon(), &tip, tint) {
                self.tool = *tool;
            }
            y += 38.0;
        }
        // the tool options, shown and hidden. The brush's size and the step the arrows use
        // live in that panel now, so the rail no longer carries a number that could only be
        // read; what is left here is the switch that brings it back.
        let tip = if self.options { tr("Hide the tool options") } else { tr("Tool options") };
        if self.ui.icon_button("tool-options", Vec2::new(r.center().x, r.bottom() - 22.0), 14.0, "tune", &tip) {
            self.options = !self.options;
        }
    }

    /// The tool options: what the tool in hand can be told to do, beside the button that chose
    /// it. Only what the tool actually has - a brush has a radius, an object tool has not.
    fn options_flyout(&mut self, session: &mut Session, r: Rect) {
        self.ui.panel(r);
        let inner = Rect::new(r.x + 12.0, r.y + 10.0, r.w - 24.0, r.h - 20.0);
        let mut y = inner.y;
        self.ui.text_in(&tr("Tool settings"), Rect::new(inner.x, y, inner.w, 18.0), 10.5, Weight::Bold, TEXT_DIM, Align::Left);
        y += 22.0;

        // the tile tool's own: what its clicks have left waiting, and the button that writes it.
        // Here and not only on the tiles page, because this panel is beside the button that chose
        // the tool - the eye is already here after adding a tile on the map
        if self.tool == Tool::Tiles {
            y += self.pending_tiles(session, "opt-write", Vec2::new(inner.x, y), inner.w);
        }

        if self.tool.is_brush() {
            self.ui.text_in(&tr("Brush radius"), Rect::new(inner.x, y, inner.w, 16.0), 11.0, Weight::Medium, TEXT_SOFT, Align::Left);
            y += 18.0;
            // the wheel over the map grows the same brush, and the slider follows it: both
            // read the session, which is where its size lives
            let mut v = session.brush() as f32;
            if self.ui.slider("opt-brush", Rect::new(inner.x, y, inner.w, 26.0), &mut v, BRUSH_MIN as f32, BRUSH_MAX as f32, 0.5, "", &|v| format!("{} m", metres(v as f64))) {
                session.set_brush(v as f64);
            }
            y += 30.0;
        }

        // the place tool is the one tool with two things it can do, so it is the one that has
        // to say which: a copy of the selection, or a file off the assets page
        if self.tool == Tool::Place {
            self.ui.text_in(&tr("Putting down"), Rect::new(inner.x, y, inner.w, 16.0), 11.0, Weight::Medium, TEXT_SOFT, Align::Left);
            y += 18.0;
            match session.armed_asset().map(str::to_string) {
                Some(sco) => {
                    let chip = Rect::new(inner.x, y, inner.w, 28.0);
                    self.ui.p().rounded(chip, 6.0, ACCENT.alpha(0.14));
                    self.ui.text_in(file_of(&sco), Rect::new(chip.x + 8.0, chip.y, chip.w - 32.0, chip.h), 11.0, Weight::Medium, ACCENT, Align::Left);
                    let tip = tr("Let it go: a click copies the selection again");
                    if self.ui.icon_button_in("opt-clear", Vec2::new(chip.right() - 13.0, chip.center().y), 9.5, "close", &tip, None) {
                        session.arm_asset(None);
                    }
                }
                None => {
                    let label = tr("Choose a .sco to put down");
                    if self.ui.button("opt-assets", Rect::new(inner.x, y, inner.w, 28.0), &label, Some("add"), ButtonKind::Ghost) {
                        self.dock = Dock::Assets;
                    }
                }
            }
            y += 32.0;
        }

        let steps: Vec<String> = STEPS.iter().map(|s| format!("{} m", metres(*s))).collect();
        self.ui.text_in(&tr("Step"), Rect::new(inner.x, y, inner.w, 16.0), 11.0, Weight::Medium, TEXT_SOFT, Align::Left);
        y += 18.0;
        let mut k = self.step.min(STEPS.len() - 1);
        if self.ui.select("opt-step", Rect::new(inner.x, y, inner.w, 28.0), &mut k, &steps) {
            self.step = k;
        }
        y += 32.0;

        let turns: Vec<String> = TURN_STEPS.iter().map(|s| format!("{}°", degrees(*s))).collect();
        self.ui.text_in(&tr("Turn step"), Rect::new(inner.x, y, inner.w, 16.0), 11.0, Weight::Medium, TEXT_SOFT, Align::Left);
        y += 18.0;
        let mut k = self.turn_step.min(TURN_STEPS.len() - 1);
        if self.ui.select("opt-turn", Rect::new(inner.x, y, inner.w, 28.0), &mut k, &turns) {
            self.turn_step = k;
        }
    }

    /// The right-hand dock: one of four panels under a strip of tabs.
    ///
    /// The panels were one, and it only ever answered one question. A map editor is asked
    /// "which object?" far more often than "what is this object?", and the console's `sel` was
    /// the only answer to it.
    fn dock(&mut self, session: &mut Session, info: &Info, r: Rect) {
        self.ui.p().rect(r, PANEL);
        self.ui.p().rect(Rect::new(r.x, r.y, 1.0, r.h), EDGE);
        self.ui.solid(r);

        let strip = Rect::new(r.x + 12.0, r.y + 10.0, r.w - 24.0, 30.0);
        // the names are translated before the call: the strip takes one string per tab, and a
        // `format!`ed label would put the English in the translation table
        let names = Dock::ALL.map(|d| tr(d.name()));
        let refs: Vec<&str> = names.iter().map(|s| &**s).collect();
        let mut open = Dock::ALL.iter().position(|d| *d == self.dock).unwrap_or(0);
        if self.ui.segmented("dock-tabs", strip, &mut open, &refs) {
            self.dock = Dock::ALL[open];
        }

        let body = Rect::new(r.x + 14.0, strip.bottom() + 10.0, r.w - 28.0, (r.bottom() - 14.0) - (strip.bottom() + 10.0));
        match self.dock {
            Dock::Inspector => self.inspector(session, info, body),
            Dock::Outline => self.outline_tab(session, info, body),
            Dock::Assets => self.assets_tab(session, body),
            Dock::History => self.history_tab(session, body),
            Dock::Unsaved => self.unsaved_tab(session, body),
        }
    }

    /// The map's objects, filtered: what to point at, when the console's `sel` is not what you
    /// want. The list is read from the document, so it says what is really there - including
    /// what this session has moved or taken away.
    ///
    /// One tab with two halves, because "which object?" and "which tile?" are the same kind of
    /// question and a map is made of both - the objects standing on it, and the tiles it is
    /// made of.
    fn outline_tab(&mut self, session: &mut Session, info: &Info, r: Rect) {
        let names = [tr("Objects"), tr("Tiles")];
        let refs: Vec<&str> = names.iter().map(|s| &**s).collect();
        let mut which = usize::from(self.outline_tiles);
        if self.ui.segmented("outline-what", Rect::new(r.x, r.y, r.w, 26.0), &mut which, &refs) {
            self.outline_tiles = which == 1;
        }
        let body = Rect::new(r.x, r.y + 32.0, r.w, (r.bottom() - (r.y + 32.0)).max(0.0));
        if self.outline_tiles {
            self.tiles_tab(session, info, body);
        } else {
            self.objects_tab(session, body);
        }
    }

    /// The map's tiles: which ones it is made of, in the order its own list gives them, with
    /// what each holds - and the two clicks that change it.
    ///
    /// The order is shown because it matters: an entry point and a track file name a tile by
    /// its place in this list, so the number down the left is what those numbers mean.
    ///
    /// The list is the second way to change the map's tiles, not the first: the tile tool rings
    /// them on the map itself, where they are (see [`Tool::Tiles`] and `crate::tiles`). This is
    /// where a tile can be looked up by its number, and where a removal that has no place on a
    /// map - a tile with an entry point standing in it - can be read about.
    fn tiles_tab(&mut self, session: &mut Session, info: &Info, r: Rect) {
        let mut y = r.y;
        // (drawn, and its height taken from what was drawn: measuring a second time is how the
        // two come to disagree)
        let line = tr("The tile tool (T) rings the map's tiles on the map itself: click one to add it, or one the map has to take it out.");
        y += self.ui.paragraph(&line, Vec2::new(r.x, y), r.w, 11.0, Weight::Regular, TEXT_FAINT) + 8.0;
        // the tile the crosshair is over: what "add a tile" means, and it needs nothing typed
        let under = info.aim.map(|at| session.tile_at(at));
        let can = under.is_some();
        let label = tr("Add the tile under the pointer");
        if self.ui.button("tiles-add", Rect::new(r.x, y, r.w, 28.0), &label, Some("add"), if can { ButtonKind::Normal } else { ButtonKind::Ghost }) {
            match under {
                Some(tile) => self.add_tile(session, tile),
                None => self.say("The pointer is not on any ground."),
            }
        }
        y += 34.0;

        // a tile added is a tile with no file until the map is written, and a tile with no file
        // is not on the ground: what is waiting goes here, under the button that makes it wait
        y += self.pending_tiles(session, "tiles-write", Vec2::new(r.x, y), r.w) + 8.0;

        let tiles = session.tiles();
        let line = format!("{} {}", tiles.len(), tr("tiles"));
        self.ui.text_in(&line, Rect::new(r.x, y, r.w, 18.0), 10.5, Weight::Bold, TEXT_DIM, Align::Left);
        y += 20.0;

        if tiles.is_empty() {
            self.ui.paragraph(
                &tr("This map lists no tile at all. Put the crosshair on the ground and add the tile it is over."),
                Vec2::new(r.x, y),
                r.w,
                11.5,
                Weight::Regular,
                TEXT_FAINT,
            );
            return;
        }

        let list = Rect::new(r.x, y, r.w, (r.bottom() - y).max(0.0));
        let row_h = 26.0;
        let mut remove = None;
        let fresh: Vec<bool> = tiles.iter().map(|t| session.doc().new_tiles().any(|n| n == *t)).collect();
        let counts: Vec<usize> = tiles.iter().map(|t| session.objects_in(*t)).collect();
        self.ui.scroll_area("tiles-list", list, &mut |ui, v| {
            let off = (list.y - v.y).max(0.0);
            let first = (off / row_h) as usize;
            let shown = (list.h / row_h).ceil() as usize + 1;
            for i in first..(first + shown).min(tiles.len()) {
                let tile = tiles[i];
                let rr = Rect::new(v.x, v.y + i as f32 * row_h, v.w - 8.0, row_h - 2.0);
                // the number the map's own files name this tile by
                let idx = format!("{i}");
                ui.text_in(&idx, Rect::new(rr.x + 4.0, rr.y, 20.0, rr.h), 10.5, Weight::Bold, ACCENT, Align::Left);
                ui.text_in(&format!("({}, {})", tile.0, tile.1), Rect::new(rr.x + 26.0, rr.y, 76.0, rr.h), 11.5, Weight::Medium, TEXT, Align::Left);
                let what = format!("{} {}", counts[i], tr("objects"));
                ui.text_in(&what, Rect::new(rr.x + 100.0, rr.y, 66.0, rr.h), 11.0, Weight::Regular, TEXT_DIM, Align::Left);
                if fresh[i] {
                    let t = tr("not written yet");
                    let w = ui.width(&t, 10.0, Weight::Bold) + 14.0;
                    let br = Rect::new(rr.right() - 40.0 - w, rr.y + 5.0, w, 14.0);
                    ui.p().rounded(br, 4.0, WARN.alpha(0.14));
                    ui.text_in(&t, br, 10.0, Weight::Bold, WARN, Align::Center);
                }
                let tip = tr("Take this tile out of the map");
                if ui.icon_button_in(&format!("tiles-rm-{i}"), Vec2::new(rr.right() - 16.0, rr.center().y), 10.0, "close", &tip, None) {
                    remove = Some(tile);
                }
            }
            tiles.len() as f32 * row_h
        });
        if let Some(tile) = remove {
            // refused, or not: the reason a refusal gives is the whole point of it, and `act`
            // is where a session's own words are put in the console
            self.act(session, |s| s.remove_tile(tile));
            self.selection_changed();
        }
    }

    /// The map's objects, as the list the outline shows.
    fn objects_tab(&mut self, session: &mut Session, r: Rect) {
        let mut y = r.y;
        if self.ui.text_input("outline-find", Rect::new(r.x, y, r.w, 30.0), &mut self.outline_query, &tr("id or .sco"), Some("search")) {
            self.outline_key = None;
        }
        y += 36.0;
        if self.ui.toggle("outline-changed", Rect::new(r.x, y, r.w, 20.0), &mut self.outline_only_changed, &tr("Only what changed")) {
            self.outline_key = None;
        }
        y += 28.0;

        // read the list again only when what it is read from has moved
        let key = OutlineKey {
            query: self.outline_query.clone(),
            only_changed: self.outline_only_changed,
            revision: session.doc().revision(),
            objects: session.doc().object_count(),
            touched: session.doc().edits().map(|(id, _)| id).sum(),
            placed: session.doc().added().map(|a| a.id).sum(),
        };
        if self.outline_key.as_ref() != Some(&key) {
            self.outline = outline_rows(session.doc(), &key.query, key.only_changed);
            self.outline_key = Some(key);
        }

        let total = session.doc().object_count() + session.doc().added_count();
        let line = format!("{} / {} {}", self.outline.len(), total, tr("objects"));
        self.ui.text_in(&line, Rect::new(r.x, y, r.w, 18.0), 10.5, Weight::Bold, TEXT_DIM, Align::Left);
        y += 20.0;

        if self.outline.is_empty() {
            self.ui.paragraph(
                &tr("No object answers to that. Point at one and click it instead, or clear the filter."),
                Vec2::new(r.x, y),
                r.w,
                11.5,
                Weight::Regular,
                TEXT_FAINT,
            );
            return;
        }

        let list = Rect::new(r.x, y, r.w, (r.bottom() - y).max(0.0));
        let row_h = 26.0;
        let mut clicked = None;
        let rows = &self.outline;
        let chosen = session.selected_id();
        self.ui.scroll_area("outline-list", list, &mut |ui, v| {
            // only the rows the window shows: a map's tens of thousands cannot be drawn
            let off = (list.y - v.y).max(0.0);
            let first = (off / row_h) as usize;
            let shown = (list.h / row_h).ceil() as usize + 1;
            for i in first..(first + shown).min(rows.len()) {
                let row = &rows[i];
                let rr = Rect::new(v.x, v.y + i as f32 * row_h, v.w - 8.0, row_h - 2.0);
                if ui.row(&format!("outline-{}", row.id), rr, chosen == Some(row.id)) {
                    clicked = Some(row.id);
                }
                // the id in the accent the panels use for one, then the file name, then what
                // has happened to it
                let idt = format!("#{}", row.id);
                let iw = ui.width(&idt, 10.5, Weight::Bold) + 12.0;
                ui.p().rounded(Rect::new(rr.x + 4.0, rr.y + 5.0, iw, 14.0), 4.0, ACCENT.alpha(0.14));
                ui.text_in(&idt, Rect::new(rr.x + 4.0, rr.y + 5.0, iw, 14.0), 10.5, Weight::Bold, ACCENT, Align::Center);
                let tag = row.badge();
                let tw = tag.map(|(t, _)| ui.width(t, 10.0, Weight::Bold) + 14.0).unwrap_or(0.0);
                let name_x = rr.x + 4.0 + iw + 8.0;
                let name_w = (rr.right() - 6.0 - tw - 6.0 - name_x).max(10.0);
                let name_c = if row.deleted { TEXT_FAINT } else { TEXT };
                ui.text_in(&row.name, Rect::new(name_x, rr.y, name_w, rr.h), 11.5, Weight::Regular, name_c, Align::Left);
                if let Some((t, danger)) = tag {
                    let c = if danger { DANGER } else { ACCENT_2 };
                    let br = Rect::new(rr.right() - 6.0 - tw, rr.y + 5.0, tw, 14.0);
                    ui.p().rounded(br, 4.0, c.alpha(0.14));
                    ui.text_in(t, br, 10.0, Weight::Bold, c, Align::Center);
                }
            }
            rows.len() as f32 * row_h
        });
        if let Some(id) = clicked {
            session.select(Some(id));
            self.selection_changed();
            // and the view goes to it: a list you cannot see the far end of is half a list
            self.go_to = session.doc().position(id);
        }
    }

    /// The `.sco` files the content folder holds: what a brand-new object is made of.
    ///
    /// The one list in these panels that is not read from the map. Every other one answers a
    /// question about what is already there; this answers "what is there to put down?", and an
    /// object can be put into a map that no other map has ever heard of (see
    /// [`omsi_editor_core::Session::place_asset`]). So it reads the folder a map is built out
    /// of - `Sceneryobjects`, in the installation and in every content folder over it.
    ///
    /// A click arms the place tool with that file and puts the tool in hand, so the next click
    /// on the ground is that object. A click on the armed row lets it go, and the place tool
    /// goes back to copying the selection.
    fn assets_tab(&mut self, session: &mut Session, r: Rect) {
        // the walk is a whole folder tree, thousands of files deep, so it is done once and
        // kept - this page is drawn sixty times a second
        if self.assets.is_none() {
            let all = omsi_editor_core::assets::scenery_objects(session.doc().install_root());
            let shown = all.clone();
            self.assets = Some(AssetList { all, shown, query: String::new() });
        }

        let mut y = r.y;
        let find = Rect::new(r.x, y, (r.w - 32.0).max(40.0), 30.0);
        self.ui.text_input("assets-find", find, &mut self.asset_query, &tr("name or folder"), Some("search"));
        let tip = tr("Look through the folders again");
        if self.ui.icon_button("assets-refresh", Vec2::new(r.right() - 6.0, find.center().y), 11.0, "autorenew", &tip) {
            // (the folder is read again on the next frame, which is one line of code where
            // asking for a redraw would be a whole path through the window)
            self.assets = None;
            return;
        }
        y += 36.0;

        // what the place tool is holding, seen: a row is a name and a folder, and neither says
        // what a `.sco` looks like - so the picture is here, above the list it came out of
        y += self.asset_preview(session, Vec2::new(r.x, y), r.w) + 8.0;

        {
            let list = self.assets.as_mut().expect("the list was just read");
            if list.query != self.asset_query {
                let needle = self.asset_query.trim().to_ascii_lowercase();
                list.shown = list
                    .all
                    .iter()
                    .filter(|s| needle.is_empty() || s.to_ascii_lowercase().contains(&needle))
                    .cloned()
                    .collect();
                list.query = self.asset_query.clone();
            }
        }

        let (all, shown) = self.assets.as_ref().map(|l| (l.all.len(), l.shown.len())).unwrap_or((0, 0));
        let line = if shown == all {
            format!("{all} {}", tr("scenery objects"))
        } else {
            format!("{shown} / {all} {}", tr("scenery objects"))
        };
        self.ui.text_in(&line, Rect::new(r.x, y, r.w, 18.0), 10.5, Weight::Bold, TEXT_DIM, Align::Left);
        y += 20.0;

        if all == 0 {
            self.ui.paragraph(
                &tr("There is no Sceneryobjects folder here, so there is nothing to build a map out of."),
                Vec2::new(r.x, y),
                r.w,
                11.5,
                Weight::Regular,
                TEXT_FAINT,
            );
            return;
        }

        if shown == 0 {
            self.ui.paragraph(
                &tr("No scenery object here answers to that."),
                Vec2::new(r.x, y),
                r.w,
                11.5,
                Weight::Regular,
                TEXT_FAINT,
            );
            return;
        }

        let list = Rect::new(r.x, y, r.w, (r.bottom() - y).max(0.0));
        let row_h = 26.0;
        let mut clicked: Option<String> = None;
        let rows = &self.assets.as_ref().expect("the list was just read").shown;
        let held = session.armed_asset().map(str::to_string);
        self.ui.scroll_area("assets-list", list, &mut |ui, v| {
            // only the rows the window shows: a full installation's are in the thousands
            let off = (list.y - v.y).max(0.0);
            let first = (off / row_h) as usize;
            let shown = (list.h / row_h).ceil() as usize + 1;
            for i in first..(first + shown).min(rows.len()) {
                let row = &rows[i];
                let rr = Rect::new(v.x, v.y + i as f32 * row_h, v.w - 8.0, row_h - 2.0);
                let armed = held.as_deref() == Some(row.as_str());
                if ui.row(&format!("asset-{i}"), rr, armed) {
                    clicked = Some(row.clone());
                }
                if armed {
                    ui.icon("check", Vec2::new(rr.x + 11.0, rr.center().y), 9.0, ACCENT);
                }
                // the file, then the folder it came out of: two scenery objects of the same
                // name are common, and the folder is what tells one from the other
                let name = file_of(row);
                let name_x = rr.x + 24.0;
                let nw = ui.width(name, 11.5, Weight::Regular) + 8.0;
                ui.text_in(name, Rect::new(name_x, rr.y, nw, rr.h), 11.5, Weight::Regular, if armed { ACCENT } else { TEXT }, Align::Left);
                let folder = folder_of(row);
                ui.text_in(folder, Rect::new(name_x + nw, rr.y, (rr.right() - 6.0 - name_x - nw).max(10.0), rr.h), 10.5, Weight::Regular, TEXT_FAINT, Align::Left);
            }
            rows.len() as f32 * row_h
        });

        if let Some(sco) = clicked {
            let held = session.armed_asset().map(str::to_string);
            if held.as_deref() == Some(sco.as_str()) {
                session.arm_asset(None);
                self.say(tr("Let go of it: a click copies the selection again"));
            } else {
                session.arm_asset(Some(sco.clone()));
                // and the tool goes to hand: choosing what to put down and then having to go
                // and find the tool for it would be a second step for nothing
                self.tool = Tool::Place;
                self.say(format!("{} {}", file_of(&sco), tr("is in the place tool")));
            }
        }
    }

    /// What has been done this session, newest first. Every step of it can be taken back, so a
    /// click on a row takes back to it.
    fn history_tab(&mut self, session: &mut Session, r: Rect) {
        let mut y = r.y;
        let bw = (r.w - 8.0) * 0.5;
        let can_undo = session.history().can_undo();
        if self
            .ui
            .button("hist-back", Rect::new(r.x, y, bw, 26.0), &tr("Take back"), Some("turn_left"), if can_undo { ButtonKind::Normal } else { ButtonKind::Ghost })
            && can_undo
        {
            self.history_back = Some(1);
        }
        let can_redo = session.history().can_redo();
        if self
            .ui
            .button("hist-fwd", Rect::new(r.x + bw + 8.0, y, bw, 26.0), &tr("Do again"), Some("turn_right"), if can_redo { ButtonKind::Normal } else { ButtonKind::Ghost })
            && can_redo
        {
            self.history_forward = Some(1);
        }
        y += 34.0;

        let depth = session.history().depth();
        let line = format!("{} {}", depth, tr("steps done"));
        self.ui.text_in(&line, Rect::new(r.x, y, r.w, 18.0), 10.5, Weight::Bold, TEXT_DIM, Align::Left);
        y += 20.0;

        if depth == 0 {
            // why there is nothing here is worth saying: a list that empties itself after a
            // save looks like a bug otherwise
            self.ui.paragraph(
                &tr("Nothing to take back. A save drops the list: the steps before it were measured against a map that is no longer the one on disk."),
                Vec2::new(r.x, y),
                r.w,
                11.5,
                Weight::Regular,
                TEXT_FAINT,
            );
        } else {
            let list = Rect::new(r.x, y, r.w, (r.bottom() - y).max(0.0));
            let row_h = 24.0;
            let mut back = None;
            let history = session.history();
            self.ui.scroll_area("hist-list", list, &mut |ui, v| {
                let off = (list.y - v.y).max(0.0);
                let first = (off / row_h) as usize;
                let shown = (list.h / row_h).ceil() as usize + 1;
                for i in first..(first + shown).min(depth) {
                    // newest first: the row at the top is the last thing done
                    let rr = Rect::new(v.x, v.y + i as f32 * row_h, v.w - 8.0, row_h - 1.0);
                    let label = history.entries().nth(depth - 1 - i).map(|c| c.label()).unwrap_or_default();
                    if ui.row(&format!("hist-{i}"), rr, false) {
                        // taking back to this step is taking back this one and every younger
                        back = Some(i + 1);
                    }
                    let n = depth - i;
                    let num = format!("{n}.");
                    let nw = ui.width(&num, 10.5, Weight::Regular) + 4.0;
                    let c = if i == 0 { TEXT } else { TEXT_DIM };
                    ui.text_in(&num, Rect::new(rr.x + 6.0, rr.y, nw, rr.h), 10.5, Weight::Regular, TEXT_FAINT, Align::Left);
                    ui.text_in(&label, Rect::new(rr.x + 6.0 + nw, rr.y, rr.w - 12.0 - nw, rr.h), 11.5, Weight::Regular, c, Align::Left);
                }
                depth as f32 * row_h
            });
            self.history_back = back.or(self.history_back);
        }

        // applied here, once the list that was clicked is no longer borrowed
        if let Some(n) = self.history_forward.take() {
            for _ in 0..n {
                match session.redo() {
                    Ok(Some(_)) => {}
                    Ok(None) => break,
                    Err(e) => {
                        self.say(format!("{e}"));
                        break;
                    }
                }
            }
            self.variants_for = None;
        }
        if let Some(n) = self.history_back.take() {
            for _ in 0..n {
                match session.undo() {
                    Ok(Some(_)) => {}
                    Ok(None) => break,
                    Err(e) => {
                        self.say(format!("{e}"));
                        break;
                    }
                }
            }
            self.variants_for = None;
        }
    }

    /// What a save would write: where it would go, and the tiles it would touch with what
    /// changed in each. The top bar says how many there are; this says which.
    fn unsaved_tab(&mut self, session: &mut Session, r: Rect) {
        let mut y = r.y;
        // read in a block of its own: what a save would write, and where. The borrow of the
        // document ends here, so the tab can go on to act on the session.
        let (rows, destination) = {
            let doc = session.doc();
            let dirty = doc.dirty_tiles();
            // (the tile, objects changed, copies placed, whether the ground was shaped)
            let mut per: std::collections::BTreeMap<(i32, i32), (usize, usize, bool)> = std::collections::BTreeMap::new();
            for t in &dirty {
                per.insert(*t, (0, 0, doc.ground_is_dirty(*t)));
            }
            for (id, e) in doc.edits() {
                if e.is_untouched() {
                    continue;
                }
                if let Some(v) = doc.object(id).and_then(|o| per.get_mut(&o.tile)) {
                    v.0 += 1;
                }
            }
            for a in doc.added() {
                if let Some(v) = per.get_mut(&a.tile) {
                    v.1 += 1;
                }
            }
            let rows: Vec<((i32, i32), (usize, usize, bool))> = per.into_iter().collect();
            (rows, doc.destination().clone())
        };

        let where_ = match &destination {
            Destination::Content(p) => format!("{}\n{}", tr("A save writes copies under"), p.display()),
            Destination::InPlace => tr("A save writes the map's own files, with a .openomsi-bak snapshot each").to_string(),
        };
        y += self.ui.paragraph(&where_, Vec2::new(r.x, y), r.w, 11.0, Weight::Regular, TEXT_FAINT) + 8.0;

        let can = !rows.is_empty();
        let kind = if can { ButtonKind::Primary } else { ButtonKind::Ghost };
        if self.ui.button("unsaved-save", Rect::new(r.x, y, r.w, 28.0), &tr("Write it now"), Some("save"), kind) && can {
            self.save(session);
        }
        y += 36.0;

        if rows.is_empty() {
            self.ui.paragraph(&tr("Nothing is waiting to be written."), Vec2::new(r.x, y), r.w, 11.5, Weight::Regular, TEXT_FAINT);
            return;
        }

        let list = Rect::new(r.x, y, r.w, (r.bottom() - y).max(0.0));
        let row_h = 24.0;
        self.ui.scroll_area("unsaved-list", list, &mut |ui, v| {
            let off = (list.y - v.y).max(0.0);
            let first = (off / row_h) as usize;
            let shown = (list.h / row_h).ceil() as usize + 1;
            for i in first..(first + shown).min(rows.len()) {
                let ((tx, ty), (objects, placed, ground)) = rows[i];
                let rr = Rect::new(v.x, v.y + i as f32 * row_h, v.w - 8.0, row_h - 2.0);
                ui.text_in(&format!("({tx}, {ty})"), Rect::new(rr.x + 4.0, rr.y, 70.0, rr.h), 11.5, Weight::Medium, TEXT, Align::Left);
                let mut what: Vec<String> = Vec::new();
                if objects > 0 {
                    what.push(format!("{objects} {}", tr("objects")));
                }
                if placed > 0 {
                    what.push(format!("{placed} {}", tr("placed")));
                }
                if ground {
                    what.push(tr("ground").to_string());
                }
                ui.text_in(&what.join(" · "), Rect::new(rr.x + 78.0, rr.y, rr.w - 82.0, rr.h), 11.0, Weight::Regular, TEXT_DIM, Align::Left);
            }
            rows.len() as f32 * row_h
        });
    }

    /// What is chosen: what it is, where it stands, and what can be done to it.
    fn inspector(&mut self, session: &mut Session, info: &Info, r: Rect) {
        let inner = r;
        let Some(sel) = session.selection() else {
            self.heading(inner, "Chosen object", None);
            self.ui.paragraph(
                "Point at something and click, or type `sel <id>` below.",
                Vec2::new(inner.x, inner.y + 34.0),
                inner.w,
                12.0,
                Weight::Regular,
                TEXT_FAINT,
            );
            return;
        };

        let mut y = self.heading(inner, "Chosen object", Some(&sel.id.to_string()));
        if sel.edit.deleted {
            let _ = self.ui.badge(Vec2::new(inner.x, y), "taken away", DANGER);
            y += 22.0;
        }
        y = field(&mut self.ui, inner, y, "model", &sel.name(), 12.5, TEXT);
        let folder = sel.object.folder().to_string_lossy().to_string();
        y = field(&mut self.ui, inner, y, "", &folder, 11.0, TEXT_FAINT);
        y += 4.0;

        let p = sel.position();
        y = self.readout_row(inner, y, "position", &[metres(p.x), metres(p.y), metres(p.z)]);
        y = self.readout_row(inner, y, "heading", &[format!("{}°", degrees(sel.heading()))]);
        y = field(
            &mut self.ui,
            inner,
            y,
            "tile",
            &format!("({}, {})", sel.object.tile.0, sel.object.tile.1),
            11.5,
            TEXT_SOFT,
        );
        y += 2.0;

        // the buttons that act on it
        let bw = (inner.w - 8.0) * 0.5;
        let row = |y: f32| Rect::new(inner.x, y, bw, 28.0);
        let aim = info.aim;
        let a = row(y);
        if self.ui.button("ins-move-aim", a, "Move to aim", None, ButtonKind::Normal) {
            match aim {
                Some(at) => self.act(session, |s| s.move_selected_on_the_ground(at)),
                None => self.say("The pointer is not on any ground."),
            }
        }
        let b = Rect::new(a.right() + 8.0, y, bw, 28.0);
        if self.ui.button("ins-face-aim", b, "Face aim", None, ButtonKind::Normal) {
            match aim {
                Some(at) => {
                    let (dx, dy) = (at.x - p.x, at.y - p.y);
                    // heading 0 is north (+y) and grows clockwise, as the map's headings do
                    let deg = dx.atan2(dy).to_degrees();
                    self.act(session, |s| s.turn_selected_to(deg));
                }
                None => self.say("The pointer is not on any ground."),
            }
        }
        y += 34.0;

        let a = row(y);
        let away = !sel.edit.deleted;
        if self.ui.button("ins-away", a, if away { "Take away" } else { "Put back" }, None, if away { ButtonKind::Danger } else { ButtonKind::Normal }) {
            self.act(session, |s| s.set_selected_deleted(away));
        }
        let b = Rect::new(a.right() + 8.0, y, bw, 28.0);
        if self.ui.button("ins-reset", b, "Put it back as it was", None, ButtonKind::Ghost) {
            self.act(session, |s| s.reset_selected());
        }
        y += 36.0;

        // a `.sco` can only be changed on an object placed in this session; a map object's
        // record has no field for it yet, so the list is not offered for one
        if sel.placed {
            if self.variants_for != Some(sel.id) {
                self.variants = session.sibling_variants();
                self.variants_for = Some(sel.id);
            }
            if self.variants.len() > 1 {
                let heads = self.heading(inner, "Variant", None);
                let mut chosen = self.variants.iter().position(|v| v.eq_ignore_ascii_case(&sel.name())).unwrap_or(0);
                let list = self.variants.clone();
                if self.ui.select("ins-variant", Rect::new(inner.x, heads, inner.w, 30.0), &mut chosen, &list) {
                    let sco = list[chosen].clone();
                    self.act(session, |s| s.set_selected_variant(sco));
                }
            }
        }
        let _ = y;
    }

    /// The line along the bottom: where the crosshair is, what the camera is doing, and what
    /// the tool will do.
    ///
    /// Read-only, deliberately. The step and the brush used to be chips on this line, and a
    /// bar of numbers is the last place anyone looks for a setting - they are in the tool
    /// options now, beside the tool they belong to.
    fn status_bar(&mut self, view: &View, info: &Info, armed: bool, r: Rect) {
        self.ui.p().rect(r, RAIL);
        self.ui.p().rect(Rect::new(r.x, r.y, r.w, 1.0), EDGE);
        self.ui.solid(r);
        let mid = r.center().y;
        let mut x = 14.0;

        let aim = match info.aim {
            Some(p) => format!("{}  {}, {}, {}", tr("pointer"), pos(p.x), pos(p.y), pos(p.z)),
            None => "pointer is not on the ground".to_string(),
        };
        x += self.status_text(x, mid, &aim, if info.aim.is_some() { TEXT_SOFT } else { TEXT_FAINT }) + 18.0;
        let tile = openomsi_game::host::tile_of(view.camera().position);
        x += self.status_text(x, mid, &format!("{} ({}, {})", tr("tile"), tile.0, tile.1), TEXT_SOFT) + 18.0;
        x += self.status_text(x, mid, &format!("{} {}", info.loaded_tiles, tr("tiles loaded")), TEXT_SOFT) + 18.0;
        x += self.status_text(x, mid, &format!("{:.0} fps", info.fps), TEXT_FAINT) + 18.0;
        let speed = format!("{} {}", tr("camera"), metres(view.speed));
        let _ = self.status_text(x, mid, &speed, TEXT_SOFT);

        // what the tool does, at the far end where nothing else goes - or, with a hand on the
        // gizmo, what that hand is on. The line is in the tool's own colour, the one its
        // outline rings an object in: green while a click would put a copy down, red while it
        // would take one away, amber for the rest.
        let (hint, colour) = match &info.holding {
            Some(name) => (tr(name), ACCENT),
            None => (tr(self.tool.hint_holding(armed)), self.tool.colour()),
        };
        let hint_w = self.ui.width(&hint, 11.5, Weight::Medium);
        self.ui.text_in(&hint, Rect::new(r.right() - 16.0 - hint_w - 4.0, r.y, hint_w + 4.0, r.h), 11.5, Weight::Medium, colour, Align::Right);
    }

    fn status_text(&mut self, x: f32, mid: f32, text: &str, c: Color) -> f32 {
        let w = self.ui.width(text, 11.5, Weight::Regular);
        let r = Rect::new(x, mid - 9.0, w + 2.0, 18.0);
        self.ui.text_in(text, r, 11.5, Weight::Regular, c, Align::Left);
        w
    }

    /// The prompt, and what it has said.
    fn console(&mut self, session: &mut Session, r: Rect) {
        self.ui.panel(r);
        self.ui.solid(r);
        let title = Rect::new(r.x + 14.0, r.y, 160.0, 28.0);
        self.ui.text_in("Commands", title, 12.0, Weight::Bold, TEXT_SOFT, Align::Left);
        let fold = if self.open { "expand_more" } else { "expand_less" };
        let tip = if self.open { "Fold the console away" } else { "Open the console" };
        if self.ui.icon_button("console-fold", Vec2::new(r.right() - 18.0, r.y + 14.0), 12.0, fold, tip) {
            self.open = !self.open;
        }
        if !self.open {
            // folded: the last line still shows what happened
            let last = self.log.last().cloned().unwrap_or_default();
            let w = (r.w - 60.0).max(0.0);
            self.ui.text_in(&last, Rect::new(r.x + 110.0, r.y, w, 28.0), 11.5, Weight::Regular, TEXT_FAINT, Align::Left);
            return;
        }

        let field = Rect::new(r.x + 12.0, r.bottom() - 38.0, r.w - 24.0, 28.0);
        let log = Rect::new(r.x + 12.0, r.y + 32.0, r.w - 24.0, (field.y - 6.0) - (r.y + 32.0));
        // `Ui::text` puts the *baseline* where it is told, so a line is drawn a line lower
        // than its own top - and a Chinese glyph is taller than a Latin one at the same size
        let line_h = 16.0;
        let show = ((log.h / line_h).floor() as usize).max(1);
        let start = self.log.len().saturating_sub(show);
        let lines: Vec<String> = self.log[start..].to_vec();
        self.ui.push_clip(log, 6.0);
        let mut y = log.y + line_h - 2.0;
        for l in &lines {
            self.ui.text(l, Vec2::new(log.x + 2.0, y), 11.5, Weight::Regular, TEXT_DIM, Align::Left);
            y += line_h;
        }
        self.ui.pop_clip();

        // Enter hands the line to the same commands the terminal took
        let id = id_of("console-line");
        let had_focus = self.ui.focus == Some(id);
        let entered = had_focus && self.ui.input.keys.contains(&Key::Enter);
        self.ui.text_input("console-line", field, &mut self.command, "sel 1687 · mv 2.5 0 0 · save", Some("terminal"));
        if entered {
            let line = std::mem::take(&mut self.command);
            self.run(session, &line);
        }
    }

    /// One line typed at the console: the prompt the editor always had, now inside the window.
    pub fn run(&mut self, session: &mut Session, line: &str) {
        let line = line.trim();
        if line.is_empty() {
            return;
        }
        self.say(format!("> {line}"));
        // `maps`, like `quit`, is about the program rather than the map, so it is answered
        // here rather than in the terminal's own command set - which has no list of maps to
        // go back to and no window to show one in. The top bar's arrow does the same.
        if line.eq_ignore_ascii_case("maps") {
            self.back = true;
            return;
        }
        match repl::run_line(session, line) {
            Ok(o) => {
                self.say(o.message);
                if o.flow == repl::Flow::Quit {
                    // `quit` is the one command about the program rather than the map: the
                    // window reads this and closes, so it still means what it always meant
                    self.quit = true;
                }
                // `tile add` and `tile rm` are lines like any other and go through the
                // terminal's own command set, so this is one of the places a tile list can
                // change without a panel being touched (see [`Panels::tiles_changed`])
                self.tiles_changed = true;
            }
            Err(e) => self.say(format!("? {e}")),
        }
        // the selection may have moved on: the variant list follows it next frame
        self.variants_for = None;
    }

    fn save(&mut self, session: &mut Session) {
        match session.save() {
            Ok(report) => {
                // a save is what puts a tile this session added on the disk, and a tile that
                // is not on the disk is a tile no world can build: the frame after a save is
                // the one a new tile's ground appears in (see [`Panels::tiles_changed`])
                self.tiles_changed = true;
                self.say(report.describe());
            }
            Err(e) => self.say(format!("{}: {e}", tr("could not save"))),
        }
    }

    fn undo(&mut self, session: &mut Session, back: bool) {
        let r = if back { session.undo() } else { session.redo() };
        match r {
            Ok(Some(m)) => self.say(m),
            Ok(None) => {}
            Err(e) => self.say(format!("{e}")),
        }
        // a step back can be a tile added or a tile taken out, and the map's list of tiles
        // to hand over is the one this leaves behind
        self.tiles_changed = true;
        self.variants_for = None;
    }

    /// Do something to the session and put what it says in the console - one place, so no
    /// panel action can fail silently.
    pub fn act(&mut self, session: &mut Session, f: impl FnOnce(&mut Session) -> Result<Option<String>, omsi_editor_core::EditError>) {
        match f(session) {
            Ok(Some(m)) => self.say(m),
            Ok(None) => {}
            Err(e) => self.say(format!("{e}")),
        }
        // whatever it was, the window compares the map's list of tiles with the one the world
        // has - and a comparison is cheaper than working out which actions can change it (see
        // [`Panels::tiles_changed`])
        self.tiles_changed = true;
    }

    /// A panel heading: the small capitalised label every panel starts with. `chip` is a note
    /// beside it (an object's id, a count).
    fn heading(&mut self, inner: Rect, title: &str, chip: Option<&str>) -> f32 {
        let y = inner.y;
        let r = Rect::new(inner.x, y, inner.w, 20.0);
        self.ui.text_in(title, r, 10.5, Weight::Bold, TEXT_DIM, Align::Left);
        if let Some(c) = chip {
            let w = self.ui.width(c, 10.5, Weight::Bold) + 12.0;
            let cr = Rect::new(inner.right() - w, y, w, 20.0);
            self.ui.p().rounded(cr, 4.0, ACCENT.alpha(0.14));
            self.ui.text_in(c, cr, 10.5, Weight::Bold, ACCENT, Align::Center);
        }
        y + 22.0
    }

    /// A label and its value: `position   195.84   193.34   0.00`.
    fn readout_row(&mut self, inner: Rect, y: f32, label: &str, values: &[String]) -> f32 {
        let lw = 62.0;
        self.ui.text_in(label, Rect::new(inner.x, y, lw, 24.0), 10.5, Weight::Bold, TEXT_DIM, Align::Left);
        let n = values.len().max(1);
        let gap = 6.0;
        let fw = (inner.w - lw - gap * (n as f32 - 1.0)) / n as f32;
        for (i, v) in values.iter().enumerate() {
            let r = Rect::new(inner.x + lw + (fw + gap) * i as f32, y, fw, 24.0);
            self.ui.p().rounded(r, 6.0, FIELD);
            self.ui.text_in(v, r, 12.0, Weight::Regular, TEXT, Align::Center);
        }
        y + 30.0
    }

    /// What is waiting to be written about the map's tiles, and the button that writes it.
    ///
    /// A tile is staged like everything else here: adding one changes the map's own list and
    /// leaves the *file* to a save - and the file is what a tile is built from. So a tile just
    /// added is listed, is chosen, is ringed on the map in red, and is not on the ground. The row
    /// it takes on this page says "not written yet" and nothing said what to do about it, which
    /// left working out that `save` was the answer as the only way to see a tile you had just
    /// added.
    ///
    /// This is that answer, where the tiles are: on this page, and in the tile tool's own options
    /// - which is where the eye already is after clicking the map with the tile tool in hand.
    /// Writing is the map's own save, so it writes whatever else is waiting too, which is what the
    /// note under the count says.
    ///
    /// Returns the height it drew, and nothing at all when nothing is waiting.
    fn pending_tiles(&mut self, session: &mut Session, id: &str, at: Vec2, w: f32) -> f32 {
        let Some(what) = waiting_tiles(session) else { return 0.0 };
        let mut y = at.y;
        y += self.ui.paragraph(&what, at, w, 11.0, Weight::Medium, WARN);
        let note = tr("A tile is on the ground once the map is written.");
        y += self.ui.paragraph(&note, Vec2::new(at.x, y), w, 11.0, Weight::Regular, TEXT_FAINT) + 6.0;
        if self.ui.button(id, Rect::new(at.x, y, w, 28.0), &tr("Write it now"), Some("save"), ButtonKind::Primary) {
            self.save(session);
        }
        y + 28.0 - at.y
    }

    /// The height [`Panels::pending_tiles`] is about to draw, which the tool options have to work
    /// their own rect out from *before* it draws. The two have to agree - the same lines at the
    /// same sizes with the same gaps - which is why they sit next to each other.
    fn pending_tiles_height(&self, session: &Session, w: f32) -> f32 {
        let Some(what) = waiting_tiles(session) else { return 0.0 };
        let note = tr("A tile is on the ground once the map is written.");
        self.ui.paragraph_height(&what, w, 11.0, Weight::Medium) + self.ui.paragraph_height(&note, w, 11.0, Weight::Regular) + 6.0 + 28.0
    }

    /// The asset the place tool is holding, drawn by the launcher's own showroom: the picture
    /// the launcher's Drive page shows of a bus, of a `.sco` instead (see
    /// `openomsi_game::host::showroom`).
    ///
    /// A row of the list below is a name and a folder, and what a scenery object *looks* like is
    /// the first thing anyone wants to know about it. Reading every `.sco` to draw a thumbnail
    /// per row would be a windowful of them at once; reading the one that was picked, once, is
    /// not. The pointer over the picture turns it and the wheel comes closer, exactly as they do
    /// on the launcher's own card.
    ///
    /// The picture itself is one frame behind: the showroom is asked for it after this layout
    /// ([`Panels::preview_picture`]), so the frame after the one that armed an asset is the first
    /// that can show it.
    ///
    /// Returns the height it drew.
    fn asset_preview(&mut self, session: &mut Session, at: Vec2, w: f32) -> f32 {
        let held = session.armed_asset().map(str::to_string);
        // what is armed is what is shown, and a look the showroom already has changes nothing -
        // so asking again every frame costs nothing
        if let Some(sco) = &held {
            let doc = session.doc();
            self.showroom.want(Look {
                root: doc.install_root().to_path_buf(),
                // the map this editor has open, named by its own path: a showroom opens a world
                // on a map, and this is where that map is
                map: doc.map_cfg().to_string_lossy().into_owned(),
                object: sco.clone(),
                // the middle of a clear day: an object is looked at in plain light, which is also
                // when the map's own season textures are at their plainest
                time: NOON,
                ..Look::default()
            });
        }

        let picture = Rect::new(at.x, at.y, w, (w * 0.72).clamp(96.0, 178.0));
        self.preview_rect = Some(picture);
        // (the pointer is over a panel here, so the map takes neither the drag nor the wheel)
        self.ui.solid(picture);
        self.ui.p().rounded(picture, 6.0, FIELD);
        match (&held, self.preview_tex, self.showroom.has_picture()) {
            (Some(_), Some(tex), true) => self.ui.image(picture, tex, 6.0),
            (Some(_), _, _) => {
                let word = if self.showroom.error.is_some() { tr("No preview") } else { tr("Loading…") };
                self.ui.text_in(&word, picture, 11.5, Weight::Regular, TEXT_FAINT, Align::Center);
            }
            // nothing is armed: what this card is for
            (None, _, _) => {
                let hint = tr("Click a scenery object to look at it");
                let th = self.ui.paragraph_height(&hint, w - 20.0, 11.0, Weight::Regular);
                self.ui.paragraph(&hint, Vec2::new(at.x + 10.0, picture.center().y - th * 0.5), w - 20.0, 11.0, Weight::Regular, TEXT_FAINT);
            }
        }
        // a word while a `.sco` is being read, where the launcher puts one
        if self.showroom.busy && self.showroom.has_picture() {
            let c = Vec2::new(picture.right() - 14.0, picture.y + 14.0);
            let a = self.ui.time * 5.0;
            self.ui.p().arc(c, 5.0, 7.0, a, a + 4.2, TEXT_SOFT);
        }
        if self.ui.hover(picture) {
            self.ui.cursor = CursorIcon::Grab;
            if self.ui.input.wheel.y.abs() > 0.0 {
                self.showroom.zoom_by((1.0 - self.ui.input.wheel.y * 0.08).clamp(0.8, 1.25));
            }
        }
        // The drag is kept here rather than in the window because the panels are what has the
        // pointer and the buttons - and because a drag that started on the button of a row must
        // not come out as an orbit once it has left it.
        if self.ui.input.pressed && picture.contains(self.ui.input.mouse) {
            self.preview_drag = Some(self.ui.input.mouse);
        }
        if !self.ui.input.down {
            self.preview_drag = None;
        }
        if let Some(last) = self.preview_drag {
            self.preview_drag = Some(self.ui.input.mouse);
            let moved = self.ui.input.mouse - last;
            if moved != Vec2::ZERO {
                self.showroom.orbit(moved.x, moved.y);
            }
        }

        // what is held, named, with the way to let it go
        let mut y = picture.bottom() + 6.0;
        if let Some(sco) = &held {
            self.ui.text_in(&tr("The place tool holds"), Rect::new(at.x, y, w, 16.0), 11.0, Weight::Medium, TEXT_SOFT, Align::Left);
            y += 18.0;
            let chip = Rect::new(at.x, y, w, 26.0);
            self.ui.p().rounded(chip, 6.0, ACCENT.alpha(0.14));
            self.ui.icon("check", Vec2::new(chip.x + 12.0, chip.center().y), 9.0, ACCENT);
            self.ui.text_in(file_of(sco), Rect::new(chip.x + 24.0, chip.y, chip.w - 46.0, chip.h), 11.5, Weight::Medium, ACCENT, Align::Left);
            let tip = tr("Let it go: a click copies the selection again");
            if self.ui.icon_button_in("assets-clear", Vec2::new(chip.right() - 14.0, chip.center().y), 10.0, "close", &tip, None) {
                session.arm_asset(None);
            }
            y += 32.0;
        }
        y - at.y
    }

    /// The showroom's picture, handed to the toolkit: made again when something about it changed,
    /// and bound into the toolkit's own texture table - which is what the card draws from, one
    /// frame later.
    ///
    /// After the layout, because how big the card is is only known once it has been laid out;
    /// and after the frame was uploaded, because the table belongs to that GPU state.
    fn preview_picture(&mut self, renderer: &mut Renderer, scale: f32) {
        let Some(card) = self.preview_rect else { return };
        let (w, h) = ((card.w * scale).round() as u32, (card.h * scale).round() as u32);
        let Some(view) = self.showroom.preview(renderer, w, h) else { return };
        let Some(gpu) = self.gpu.as_mut() else { return };
        if self.preview_gen != self.showroom.generation {
            self.preview_gen = self.showroom.generation;
            match self.preview_tex {
                Some(id) => gpu.set_view(&renderer.device, id, &view, (w, h)),
                None => self.preview_tex = Some(gpu.add_view(&renderer.device, &view, (w, h))),
            }
        }
    }

    /// Add a tile and say what it means: the map's own list has it and the ground has not, and
    /// writing the map is what puts it there (see [`Panels::pending_tiles`]). Where a tile is
    /// added from a click - the map itself or this page's button - so that neither has to
    /// remember to say it.
    pub fn add_tile(&mut self, session: &mut Session, tile: TileId) {
        self.act(session, |s| s.add_tile(tile));
        if session.doc().new_tiles().any(|t| t == tile) {
            self.say(tr("A tile is on the ground once the map is written."));
        }
    }
}

/// What the tiles page and the tool options both say about the map's tiles: how many are not
/// written yet, or that the map's own list of them has changed. `None` when there is nothing
/// waiting, which is most of the time.
fn waiting_tiles(session: &Session) -> Option<String> {
    let unwritten = session.doc().new_tiles().count();
    if unwritten > 0 {
        return Some(format!("{unwritten} {}", tr("tiles not written")));
    }
    session.doc().is_listed_dirty().then(|| tr("the tile list has changed").to_string())
}

/// A rounded chip with a coloured dot and a label; returns its width and whether it was
/// clicked. `right` is where its right edge goes.
fn chip(ui: &mut Ui, at: Vec2, text: &str, colour: Color, right: f32) -> (f32, bool) {
    let px = 11.5;
    let w = ui.width(text, px, Weight::Medium) + 34.0;
    let r = Rect::new(right - w, at.y, w, 26.0);
    let (hovered, _, clicked) = ui.interact(id_of(&format!("chip{}{}", at.y as i32, text)), r);
    ui.p().rounded(r, 6.0, if hovered { HOVER } else { FIELD });
    ui.p().rounded_border(r, 7.0, 1.0, EDGE);
    ui.p().circle(Vec2::new(r.x + 13.0, r.center().y), 3.5, colour);
    ui.text_in(text, Rect::new(r.x + 22.0, r.y, r.w - 26.0, r.h), px, Weight::Medium, TEXT_SOFT, Align::Left);
    (w, clicked)
}

/// Just the file name of a `.sco` written the way a record writes it.
fn file_of(sco: &str) -> &str {
    sco.rsplit_once('\\').map(|(_, n)| n).unwrap_or(sco)
}

/// The folder part of it (``Sceneryobjects\Berlin``), or `""` when it names no folder.
fn folder_of(sco: &str) -> &str {
    sco.rsplit_once('\\').map(|(f, _)| f).unwrap_or("")
}

/// A labelled line inside a panel: the label in capitals at the left, the value under it.
fn field(ui: &mut Ui, inner: Rect, y: f32, label: &str, value: &str, px: f32, c: Color) -> f32 {
    let mut y = y;
    if !label.is_empty() {
        ui.text_in(label, Rect::new(inner.x, y, inner.w, 18.0), 10.5, Weight::Bold, TEXT_DIM, Align::Left);
        y += 17.0;
    }
    let w = ui.paragraph_height(value, inner.w, px, Weight::Medium).min(34.0);
    ui.paragraph(value, Vec2::new(inner.x, y), inner.w, px, Weight::Medium, c);
    y + w + 8.0
}

/// A length for the panels: metres, without a trailing `.00`.
fn metres(v: f64) -> String {
    trim(format!("{v:.2}"))
}

/// A place for the panels: two places, so a column of them lines up.
fn pos(v: f64) -> String {
    format!("{v:.2}")
}

/// An angle for the panels: one place, without a trailing `.0`.
fn degrees(v: f64) -> String {
    trim(format!("{v:.1}"))
}

fn trim(s: String) -> String {
    if s.contains('.') {
        let t = s.trim_end_matches('0').trim_end_matches('.');
        t.to_string()
    } else {
        s
    }
}

/// Where a save writes when the panel asks for copies rather than the map's own files: the
/// game's own content folder, the same one the launcher and the game use.
fn default_content() -> std::path::PathBuf {
    omsi_launcher_lib::player_content_dir().unwrap_or_else(|| std::path::PathBuf::from("content"))
}
