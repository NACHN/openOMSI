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
use omsi_editor_core::{Destination, Session};
use omsi_render::Renderer;
use omsi_ui::paint::Align;
use omsi_ui::tr;
use omsi_ui::{Color, Draw, Gpu, Layer, Rect, Weight};
use openomsi_game::host::ui::{
    id_of, ButtonKind, Key, Ui, ACCENT, ACCENT_2, DANGER, EDGE, FIELD, HOVER, OK, PANEL, RAIL, SELECTED, TEXT, TEXT_DIM,
    TEXT_FAINT, TEXT_SOFT, WARN,
};

/// The bars' sizes, in logical pixels.
const TOP_H: f32 = 46.0;
const RAIL_W: f32 = 54.0;
const SIDE_W: f32 = 244.0;
const STATUS_H: f32 = 28.0;
const CONSOLE_W: f32 = 430.0;

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
    /// Put a copy of what is chosen where the crosshair meets the ground.
    Place,
    /// Take what is under the crosshair out of the map (`Restore` puts it back).
    Delete,
    /// Raise the ground under the crosshair.
    Raise,
    /// Flatten the ground under the crosshair to its own height.
    Flatten,
}

impl Tool {
    /// Every tool, in the order the rail shows them.
    pub const ALL: [Tool; 7] = [Tool::Select, Tool::Move, Tool::Turn, Tool::Place, Tool::Delete, Tool::Raise, Tool::Flatten];

    fn icon(self) -> &'static str {
        match self {
            Tool::Select => "near_me",
            Tool::Move => "open_with",
            Tool::Turn => "autorenew",
            Tool::Place => "content_copy",
            Tool::Delete => "delete",
            Tool::Raise => "arrow_upward",
            Tool::Flatten => "remove",
        }
    }

    fn name(self) -> &'static str {
        match self {
            Tool::Select => "Select",
            Tool::Move => "Move",
            Tool::Turn => "Turn",
            Tool::Place => "Place a copy",
            Tool::Delete => "Take away",
            Tool::Raise => "Raise the ground",
            Tool::Flatten => "Flatten the ground",
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
        }
    }

    /// What the mouse does with this tool - the line the status bar carries.
    ///
    /// Every one of them is a single click and so a single step to take back: a drag would be
    /// a hundred commands unless the whole gesture were made into one, which is not written
    /// yet. The arrows do the fine work.
    fn hint(self) -> &'static str {
        match self {
            Tool::Select => "Left click chooses what the crosshair is on",
            Tool::Move => "Left click puts it on the ground · drag an arrow to slide it",
            Tool::Turn => "Left click turns it to face the crosshair · drag the ring to turn it",
            Tool::Place => "Left click puts a copy where the crosshair meets the ground",
            Tool::Delete => "Left click takes away what the crosshair is on",
            Tool::Raise => "Left click raises the ground by the step · Shift lowers it",
            Tool::Flatten => "Left click levels the ground to where the crosshair is",
        }
    }

    /// The ground tools share the brush; the object tools share the selection.
    pub fn is_brush(self) -> bool {
        matches!(self, Tool::Raise | Tool::Flatten)
    }
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
            open: true,
            command: String::new(),
            log: vec![
                "Commands work here as they did in the terminal: help lists them.".to_string(),
                "The map is flown with W A S D, Q and E, the right button turns the view.".to_string(),
            ],
            variants: Vec::new(),
            variants_for: None,
            reopen: None,
            clicked_map: false,
            quit: false,
            page: start::State::default(),
            maps: Vec::new(),
            vehicles: Vec::new(),
            open_map: None,
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

    /// Set by the Maps page's "New map"; the window reads it and makes the map (or says that
    /// making one is not written yet).
    pub fn take_new_map(&mut self) -> bool {
        std::mem::take(&mut self.page.new_map)
    }

    /// Open one of the first page's three sections, as a click on the rail would - what
    /// `--ui-section` sets, so that a page can be looked at without clicking through to it.
    pub fn show_section(&mut self, section: start::Section) {
        self.page.section = section;
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

    fn step_label(&self) -> String {
        metres(self.step())
    }

    fn turn_label(&self) -> String {
        format!("{}°", degrees(self.turn_step()))
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
        renderer: &Renderer,
        size: (u32, u32),
        scale: f32,
        dt: f32,
    ) -> Frame {
        let w = size.0 as f32 / scale;
        let h = size.1 as f32 / scale;
        self.ui.begin(Vec2::new(w, h), scale, dt);
        self.layout(session, view, info, w, h);
        // a click the panels did not take is the map's. Read here and not by the window,
        // because only now are the panels laid out (`over_ui`), and because `finish` uses the
        // frame's input up - a press that started on a button must not also land on the map.
        self.clicked_map = self.ui.input.pressed && !self.ui.input.right_down && !self.ui.over_ui && self.ui.focus.is_none();
        self.upload(renderer)
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
                crate::start::Action::Quit => quit = true,
            }
        }
        if let Some(cfg) = open {
            self.open_map = Some(cfg);
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
        self.inspector(session, info, side);
        self.status_bar(session, view, info, status);
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
        self.ui.text_in(&name, Rect::new(16.0, r.y, 320.0, r.h), 14.0, Weight::Bold, TEXT, Align::Left);
        let named = self.ui.width(&name, 14.0, Weight::Bold);
        let file = session.doc().map_cfg().to_string_lossy().to_string();
        self.ui
            .text_in(&file, Rect::new(16.0 + named + 12.0, r.y, 560.0, r.h), 11.5, Weight::Regular, TEXT_FAINT, Align::Left);

        // from the right: save, redo, undo, then what a save would do and what is waiting
        let mut x = r.right() - 16.0;
        let mid = r.center().y;
        let dirty = session.doc().dirty_tiles().len();
        let can_save = dirty > 0;
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
        let (label, colour) = if dirty == 0 {
            ("No changes yet".to_string(), OK)
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
            if self.tool == *tool {
                self.ui.p().rounded(slot, 8.0, SELECTED);
                self.ui.p().rounded(Rect::new(slot.x, slot.y + 7.0, 2.0, slot.h - 14.0), 1.0, ACCENT);
            }
            // the name is translated first: the tip goes through the drawing call as one
            // string, so `format!`ing it here would put "Move (M)" in the table instead
            let tip = format!("{} ({})", tr(tool.name()), tool.key());
            if self.ui.icon_button(&format!("tool{}", tool.key()), slot.center(), 16.0, tool.icon(), &tip) {
                self.tool = *tool;
            }
            y += 38.0;
        }
        // the brush's size, while a ground tool is in hand
        if self.tool.is_brush() {
            y += 6.0;
            self.ui.text_in("m", Rect::new(r.x, y, r.w, 14.0), 10.0, Weight::Bold, TEXT_FAINT, Align::Center);
        }
    }

    /// What is chosen: what it is, where it stands, and what can be done to it.
    fn inspector(&mut self, session: &mut Session, info: &Info, r: Rect) {
        self.ui.p().rect(r, PANEL);
        self.ui.p().rect(Rect::new(r.x, r.y, 1.0, r.h), EDGE);
        self.ui.solid(r);
        let inner = Rect::new(r.x + 14.0, r.y + 12.0, r.w - 28.0, r.h - 24.0);

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
                None => self.say("The crosshair is not on any ground."),
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
                None => self.say("The crosshair is not on any ground."),
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

    /// The line along the bottom: where the crosshair is, what the camera is doing, what the
    /// tool will do, and the step the arrows use.
    fn status_bar(&mut self, session: &mut Session, view: &View, info: &Info, r: Rect) {
        self.ui.p().rect(r, RAIL);
        self.ui.p().rect(Rect::new(r.x, r.y, r.w, 1.0), EDGE);
        self.ui.solid(r);
        let mid = r.center().y;
        let mut x = 14.0;

        let aim = match info.aim {
            Some(p) => format!("{}  {}, {}, {}", tr("crosshair"), pos(p.x), pos(p.y), pos(p.z)),
            None => "crosshair is not on the ground".to_string(),
        };
        x += self.status_text(x, mid, &aim, if info.aim.is_some() { TEXT_SOFT } else { TEXT_FAINT }) + 18.0;
        let tile = openomsi_game::host::tile_of(view.camera().position);
        x += self.status_text(x, mid, &format!("{} ({}, {})", tr("tile"), tile.0, tile.1), TEXT_SOFT) + 18.0;
        x += self.status_text(x, mid, &format!("{} {}", info.loaded_tiles, tr("tiles loaded")), TEXT_SOFT) + 18.0;
        x += self.status_text(x, mid, &format!("{:.0} fps", info.fps), TEXT_FAINT) + 18.0;
        let speed = format!("{} {}", tr("camera"), metres(view.speed));
        x += self.status_text(x, mid, &speed, TEXT_SOFT) + 18.0;

        // the step the arrows and a click use, and - with a ground tool in hand - the brush
        // beside it, because a ground tool uses both
        let chip_at = |this: &mut Self, x: f32, label: String, tip: &str, id: &str| -> (f32, bool) {
            let w = this.ui.width(&label, 11.5, Weight::Medium) + 22.0;
            let r = Rect::new(x, mid - 12.0, w, 24.0);
            let (hovered, _, clicked) = this.ui.interact(id_of(id), r);
            this.ui.p().rounded(r, 6.0, if hovered { HOVER } else { FIELD });
            this.ui.text_in(&label, r, 11.5, Weight::Medium, TEXT_SOFT, Align::Center);
            this.ui.tooltip(r, tip);
            (w, clicked)
        };
        if self.tool.is_brush() {
            let brush = session.brush();
            let label = format!("{} {}", tr("brush"), metres(brush));
            let (w, clicked) = chip_at(self, x, label, "The ground brush's radius - click to change it", "status-brush");
            if clicked {
                let next = [2.0, 4.0, 6.0, 10.0, 20.0, 40.0];
                let i = next.iter().position(|v| *v > brush + 0.01).unwrap_or(0);
                session.set_brush(next[i]);
            }
            x += w + 8.0;
        }
        let step_label = format!("{} {}  ·  {} {}", tr("step"), self.step_label(), tr("turn"), self.turn_label());
        let (_, clicked) = chip_at(self, x, step_label, "How far an arrow key moves and turns - click to change", "status-step");
        if clicked {
            self.step = (self.step + 1) % STEPS.len();
            self.turn_step = (self.turn_step + 1) % TURN_STEPS.len();
        }

        // what the tool does, at the far end where nothing else goes - or, with a hand on the
        // gizmo, what that hand is on
        let (hint, colour) = match &info.holding {
            Some(name) => (tr(name), ACCENT),
            None => (tr(self.tool.hint()), TEXT_DIM),
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
        if line.trim().is_empty() {
            return;
        }
        self.say(format!("> {}", line.trim()));
        match repl::run_line(session, line) {
            Ok(o) => {
                self.say(o.message);
                if o.flow == repl::Flow::Quit {
                    // `quit` is the one command about the program rather than the map: the
                    // window reads this and closes, so it still means what it always meant
                    self.quit = true;
                }
            }
            Err(e) => self.say(format!("? {e}")),
        }
        // the selection may have moved on: the variant list follows it next frame
        self.variants_for = None;
    }

    fn save(&mut self, session: &mut Session) {
        match session.save() {
            Ok(report) => self.say(report.describe()),
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
