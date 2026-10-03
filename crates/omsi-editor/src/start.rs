//! What the editor opens on, before a map is loaded.
//!
//! It is laid out the way the launcher is: a rail of the things a person came to do, and the
//! page that one of them opens. The three are the map editor, the vehicle editor and a test
//! track - because a modder's afternoon is not always a map, and because "which map?" cannot
//! ask the question the first page actually has to ask.
//!
//! The widgets are the launcher's (see `crate::ui`), so this page is drawn with the same
//! buttons, lists and colours the player already knows. Nothing here decides *how* a map is
//! edited: a click only ever names which map to open, and the editing pages take over from
//! there (`crate::window`).

use crate::ui::MapEntry;
use glam::Vec2;
use omsi_editor_core::Destination;
use omsi_ui::paint::Align;
use omsi_ui::tr;
use omsi_ui::{Color, Rect, Weight};
use openomsi_game::host::ui::{
    id_of, ButtonKind, Ui, ACCENT, ACCENT_2, EDGE, HOVER, PANEL, RAIL, SELECTED, TEXT, TEXT_DIM, TEXT_FAINT, TEXT_SOFT,
};

/// The rail's width - the launcher's own, so the two programs open the same way.
pub const RAIL_W: f32 = 236.0;
/// A nav item: its height, and the space from one to the next.
const ITEM_H: f32 = 38.0;
const ITEM_STEP: f32 = 42.0;
/// Where the nav items start, under the title.
const ITEMS_Y: f32 = 96.0;
/// The margin around a page's own content.
const MARGIN: f32 = 28.0;

/// The three things the rail offers.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Section {
    Maps,
    Vehicles,
    Testing,
}

impl Section {
    pub const ALL: [Section; 3] = [Section::Maps, Section::Vehicles, Section::Testing];

    /// What the rail calls it.
    pub fn name(self) -> &'static str {
        match self {
            Section::Maps => "Maps",
            Section::Vehicles => "Vehicles",
            Section::Testing => "Testing Area",
        }
    }

    /// One line saying what the page is for, under the page's own title.
    pub fn hint(self) -> &'static str {
        match self {
            Section::Maps => "Build a map, or change one that is already there",
            Section::Vehicles => "A vehicle's own files: its model, its scripts, its sounds",
            Section::Testing => "Drive a vehicle, or watch an object, on a track of your own",
        }
    }

    fn icon(self) -> &'static str {
        match self {
            Section::Maps => "map",
            Section::Vehicles => "directions_bus",
            Section::Testing => "construction",
        }
    }

    /// The section a name asks for - `--ui-section maps` and the like. It answers to the
    /// short name or to the rail's own, with the spaces and the case taken out: a section
    /// called "Testing Area" is not one word, and a command line would rather not say it.
    pub fn named(name: &str) -> Option<Section> {
        let key = name.trim().to_ascii_lowercase().replace([' ', '-', '_'], "");
        Section::ALL.into_iter().find(|s| s.slug() == key || s.name().to_ascii_lowercase().replace(' ', "") == key)
    }

    /// The one-word name of each section.
    fn slug(self) -> &'static str {
        match self {
            Section::Maps => "maps",
            Section::Vehicles => "vehicles",
            Section::Testing => "testing",
        }
    }
}

/// A vehicle the editor could work on: a `.bus` or `.ovh` under a folder of `Vehicles`.
///
/// It is here for the page to be a page; nothing can be done to one yet (see
/// [`Section::Vehicles`]).
pub struct VehicleEntry {
    /// The file's own name, without the folder - what a person calls the bus.
    pub name: String,
    /// The folder of `Vehicles` it sits in, which is what its `.bus` files are found by.
    pub folder: String,
    /// Whether this copy came from the content folder rather than the installation.
    pub from_content: bool,
}

/// What the pages are drawn from: the folders of the run, and what was found in them.
pub struct Data<'a> {
    pub root: &'a std::path::Path,
    pub content: Option<&'a std::path::Path>,
    pub destination: &'a Destination,
    pub maps: &'a [MapEntry],
    pub vehicles: &'a [VehicleEntry],
}

/// The little a click has to remember from one frame to the next.
pub struct State {
    pub section: Section,
    /// The highlighted map, and the highlighted vehicle.
    pub map_choice: usize,
    pub vehicle_choice: usize,
    /// Set by the Maps page's "New map": the window makes the map and opens it (or says why
    /// it cannot yet - making one is not written).
    pub new_map: bool,
}

impl Default for State {
    fn default() -> Self {
        State { section: Section::Maps, map_choice: 0, vehicle_choice: 0, new_map: false }
    }
}

/// What the page asked for.
pub enum Action {
    /// Nothing this frame.
    None,
    /// Open the map at this index in `data.maps`.
    Open(usize),
    /// Quit: the window closes.
    Quit,
}

/// Draw the first page and say what the click meant.
pub fn draw(ui: &mut Ui, state: &mut State, data: &Data, w: f32, h: f32) -> Action {
    // the whole window is interface here: there is no map behind this page
    ui.p().rect(Rect::new(0.0, 0.0, w, h), RAIL);
    ui.solid(Rect::new(0.0, 0.0, w, h));

    let mut action = Action::None;
    match rail(ui, state, data, h) {
        Some(Action::Quit) => action = Action::Quit,
        _ => {}
    }

    let content = Rect::new(RAIL_W + MARGIN, MARGIN, (w - RAIL_W - MARGIN * 2.0).max(200.0), (h - MARGIN * 2.0).max(120.0));
    let asked = match state.section {
        Section::Maps => maps_page(ui, state, data, content),
        Section::Vehicles => vehicles_page(ui, state, data, content),
        Section::Testing => testing_page(ui, content),
    };
    if let Some(a) = asked {
        action = a;
    }
    action
}

/// The rail: what there is to do, and the run's own two folders at the foot of it.
fn rail(ui: &mut Ui, state: &mut State, data: &Data, h: f32) -> Option<Action> {
    let r = Rect::new(0.0, 0.0, RAIL_W, h);
    ui.p().rect(r, RAIL);
    ui.p().rect(Rect::new(RAIL_W - 1.0, 0.0, 1.0, h), EDGE);
    ui.solid(r);

    ui.text_in("openOMSI editor", Rect::new(24.0, 30.0, RAIL_W - 40.0, 26.0), 19.0, Weight::Bold, TEXT, Align::Left);
    ui.text_in(env!("CARGO_PKG_VERSION"), Rect::new(24.0, 54.0, RAIL_W - 40.0, 18.0), 12.0, Weight::Regular, TEXT_DIM, Align::Left);

    let mut action = None;
    let mut y = ITEMS_Y;
    for s in Section::ALL {
        let item = Rect::new(12.0, y, RAIL_W - 24.0, ITEM_H);
        let id = id_of(&format!("start-nav-{}", s.name()));
        let (hovered, _, clicked) = ui.interact(id, item);
        if clicked {
            state.section = s;
        }
        let selected = state.section == s;
        if selected {
            ui.p().rounded(item, 6.0, SELECTED);
            ui.p().rounded(Rect::new(item.x, item.y + 10.0, 2.0, item.h - 20.0), 1.0, ACCENT);
        } else if hovered {
            ui.p().rounded(item, 6.0, HOVER);
        }
        let c = if selected { TEXT } else if hovered { TEXT_SOFT } else { TEXT_DIM };
        ui.icon(s.icon(), Vec2::new(item.x + 20.0, item.center().y), 18.0, c);
        ui.text_in(&tr(s.name()), Rect::new(item.x + 40.0, item.y, item.w - 50.0, item.h), 13.5, Weight::Medium, c, Align::Left);
        y += ITEM_STEP;
    }

    // the foot of the rail: the two folders that decide what a page will find and where a
    // save goes - the one thing about the run that is true whatever page is open
    let card = Rect::new(12.0, h - 132.0, RAIL_W - 24.0, 96.0);
    if card.y > y + 8.0 {
        ui.p().rounded(card, 6.0, PANEL);
        let line = |ui: &mut Ui, dy: f32, label: &str, value: String, c: Color| {
            ui.text_in(&tr(label), Rect::new(card.x + 12.0, card.y + dy, card.w - 24.0, 16.0), 10.5, Weight::Bold, TEXT_DIM, Align::Left);
            ui.text_in(&value, Rect::new(card.x + 12.0, card.y + dy + 15.0, card.w - 24.0, 16.0), 11.0, Weight::Regular, c, Align::Left);
        };
        line(ui, 10.0, "OMSI 2", short_path(data.root, 26), TEXT_SOFT);
        line(ui, 48.0, "Writing", write_short(data.destination), TEXT_FAINT);
    }

    // and out: the window's own close button does it too, but a page that only ever has one
    // way off it is a page people get stuck on
    let quit = Rect::new(12.0, h - 32.0, RAIL_W - 24.0, 28.0);
    if quit.y > y + 4.0 && ui.button("start-quit", quit, &tr("Quit"), None, ButtonKind::Ghost) {
        action = Some(Action::Quit);
    }
    action
}

/// Where a save writes, said in the rail's few words: the full paths are on the Maps page.
fn write_short(destination: &Destination) -> String {
    match destination {
        Destination::Content(p) => format!("{} {}", tr("copies under"), short_path(p, 26)),
        Destination::InPlace => tr("the map's own files").to_string(),
    }
}

/// A path shortened to fit the rail.
///
/// The last folder is what names a place, so it is what is kept: `openOMSI-0.1.6-windows-x64`
/// says more in a narrow rail than `C:/dev/Game/openOMSI-0.1.…` does - and more than an
/// ellipsis starting in the middle of a word. Only a last folder too long for the rail is cut.
fn short_path(p: &std::path::Path, chars: usize) -> String {
    let s = p.to_string_lossy().replace('\\', "/");
    if s.chars().count() <= chars {
        return s;
    }
    let tail = s.rsplit('/').find(|part| !part.is_empty()).unwrap_or(s.as_str());
    if tail.chars().count() <= chars {
        return tail.to_string();
    }
    let mut kept: Vec<char> = tail.chars().rev().take(chars.saturating_sub(1)).collect();
    kept.reverse();
    format!("…{}", kept.into_iter().collect::<String>())
}

/// The page's own head: what it is, and one line saying so.
fn page_head(ui: &mut Ui, r: Rect, section: Section, count: Option<usize>) -> Rect {
    ui.text_in(&tr(section.name()), Rect::new(r.x, r.y, r.w - 200.0, 28.0), 20.0, Weight::Bold, TEXT, Align::Left);
    if let Some(n) = count {
        ui.text_in(&n.to_string(), Rect::new(r.x, r.y, r.w, 28.0), 13.0, Weight::Bold, TEXT_FAINT, Align::Right);
    }
    ui.text_in(&tr(section.hint()), Rect::new(r.x, r.y + 28.0, r.w - 200.0, 20.0), 12.0, Weight::Regular, TEXT_DIM, Align::Left);
    Rect::new(r.x, r.y + 60.0, r.w, (r.h - 60.0).max(0.0))
}

/// A row of the lists: a name, what it is under it, and where the copy came from.
///
/// `tag` is the right-hand end - which of the two folders this copy is in.
fn list_row(ui: &mut Ui, id: &str, r: Rect, name: &str, under: &str, from_content: bool, selected: bool) -> bool {
    let clicked = ui.row(id, r, selected);
    let tag = 150.0;
    ui.text_in(name, Rect::new(r.x + 12.0, r.y + 5.0, r.w - tag - 20.0, 16.0), 13.0, Weight::Medium, TEXT, Align::Left);
    ui.text_in(under, Rect::new(r.x + 12.0, r.y + 21.0, r.w - tag - 20.0, 14.0), 10.5, Weight::Regular, TEXT_FAINT, Align::Left);
    let (from, colour) = if from_content {
        (tr("in the content folder"), ACCENT_2)
    } else {
        (tr("in the installation"), TEXT_FAINT)
    };
    ui.text_in(&from, Rect::new(r.right() - tag, r.y, tag - 8.0, r.h), 10.5, Weight::Bold, colour, Align::Right);
    clicked
}

// ---- Maps -------------------------------------------------------------------------------

/// The map editor's page: what to build, or what to change.
///
/// The list is on the left and the chosen map has the rest of the page to itself, because
/// what someone picks a map *by* is how it looks - its roads and where its entry points
/// stand - and not its file name.
fn maps_page(ui: &mut Ui, state: &mut State, data: &Data, r: Rect) -> Option<Action> {
    let list_w = 360.0_f32.min(r.w * 0.5).max(240.0);
    let body = page_head(ui, r, Section::Maps, Some(data.maps.len()));
    let list = Rect::new(body.x, body.y + 44.0, list_w, (body.bottom() - 96.0) - (body.y + 44.0));
    let side = Rect::new(list.right() + 20.0, body.y, body.w - list_w - 20.0, body.bottom() - body.y);

    // the one button the list cannot be: a map that is not there yet
    let new = Rect::new(body.x, body.y, 132.0, 32.0);
    if ui.button("start-new-map", new, &tr("New map"), Some("add"), ButtonKind::Primary) {
        state.new_map = true;
    }

    let mut action = None;
    let mut picked: Option<usize> = None;
    let mut opened: Option<usize> = None;
    if data.maps.is_empty() {
        ui.paragraph(
            &tr("No map was found: nothing with a `global.cfg` is under the folders below."),
            Vec2::new(list.x, list.y + 8.0),
            list.w,
            12.5,
            Weight::Regular,
            TEXT_DIM,
        );
    } else {
        let rows: Vec<(String, String, bool)> =
            data.maps.iter().map(|m| (m.name.clone(), m.cfg.to_string_lossy().to_string(), m.from_content)).collect();
        let chosen = state.map_choice;
        ui.scroll_area("start-maps", list, &mut |ui, a| {
            let row_h = 40.0;
            let mut y = a.y;
            for (i, (name, cfg, from_content)) in rows.iter().enumerate() {
                let row = Rect::new(a.x, y, a.w, row_h);
                if row.bottom() > a.y - row_h && row.y < a.bottom() + row_h {
                    // choosing and opening are two clicks, as they are everywhere else: a
                    // single click that both moves the highlight and starts loading a map is
                    // a click nobody can aim
                    if list_row(ui, &format!("start-map{i}"), row, name, cfg, *from_content, chosen == i) {
                        if chosen == i {
                            opened = Some(i);
                        } else {
                            picked = Some(i);
                        }
                    }
                }
                y += row_h;
            }
            y - a.y
        });
        if let Some(i) = picked {
            state.map_choice = i;
            ui.scroll_to("start-maps", i as f32 * 40.0, 40.0, list.h);
        }
    }

    // what the files cannot show: which copy is read, which one a save writes
    let foot = Rect::new(body.x, body.bottom() - 48.0, list_w, 48.0);
    ui.p().rect(Rect::new(foot.x, foot.y, foot.w, 1.0), EDGE);
    let reading = match data.content {
        Some(c) => format!("{}  →  {}", c.display(), tr("the installation")),
        None => tr("the installation (no content folder is known)").to_string(),
    };
    ui.text_in(&tr("Reading"), Rect::new(foot.x, foot.y + 8.0, 66.0, 18.0), 11.0, Weight::Bold, TEXT_DIM, Align::Left);
    ui.text_in(&reading, Rect::new(foot.x + 66.0, foot.y + 8.0, foot.w - 66.0, 18.0), 11.0, Weight::Regular, TEXT_FAINT, Align::Left);
    ui.text_in(&tr("Writing"), Rect::new(foot.x, foot.y + 28.0, 66.0, 18.0), 11.0, Weight::Bold, TEXT_DIM, Align::Left);
    ui.text_in(&write_short(data.destination), Rect::new(foot.x + 66.0, foot.y + 28.0, foot.w - 66.0, 18.0), 11.0, Weight::Regular, TEXT_FAINT, Align::Left);

    preview(ui, side, data, state.map_choice);

    // the button the list knows nothing about: it acts on what is highlighted
    let open = Rect::new(side.right() - 132.0, side.bottom() - 36.0, 132.0, 32.0);
    let kind = if data.maps.is_empty() { ButtonKind::Ghost } else { ButtonKind::Primary };
    if ui.button("start-open", open, &tr("Edit this map"), None, kind) && !data.maps.is_empty() {
        opened = Some(state.map_choice);
    }
    if let Some(i) = opened {
        state.map_choice = i;
        action = Some(Action::Open(i));
    }
    action
}

/// The chosen map's own picture: its roads, and where its entry points stand.
///
/// It is a panel for now, and the note in it says what will be drawn - the read that fills it
/// is the one the launcher's own map does (`scene::navigation_map_of`, then the navigator's
/// roads), and that read has to be able to run with no world loaded before this can show it.
fn preview(ui: &mut Ui, r: Rect, data: &Data, choice: usize) {
    ui.panel(r);
    let inner = Rect::new(r.x + 16.0, r.y + 16.0, r.w - 32.0, r.h - 32.0);
    let head = ui.heading(Rect::new(inner.x, inner.y, inner.w, 26.0), "This map", None);
    let chosen = data.maps.get(choice);
    let name = chosen.map(|m| m.name.clone()).unwrap_or_else(|| tr("No map chosen").to_string());
    ui.text_in(&name, Rect::new(head.x, head.y, inner.w, 24.0), 15.0, Weight::Medium, TEXT, Align::Left);
    let file = chosen.map(|m| m.cfg.to_string_lossy().to_string()).unwrap_or_default();
    ui.text_in(&file, Rect::new(head.x, head.y + 22.0, inner.w, 18.0), 10.5, Weight::Regular, TEXT_FAINT, Align::Left);
    ui.paragraph(
        &tr("Its roads and its entry points are drawn here once a map is chosen."),
        Vec2::new(inner.x, head.y + 56.0),
        inner.w,
        12.0,
        Weight::Regular,
        TEXT_DIM,
    );
}

// ---- Vehicles ---------------------------------------------------------------------------

/// The vehicle editor's page. Nothing can be done to a vehicle yet; the page is the list, so
/// that what is there to work on is known before the working is written.
fn vehicles_page(ui: &mut Ui, state: &mut State, data: &Data, r: Rect) -> Option<Action> {
    let body = page_head(ui, r, Section::Vehicles, Some(data.vehicles.len()));
    if data.vehicles.is_empty() {
        ui.paragraph(
            &tr("No vehicle was found: nothing is under `Vehicles` in either folder."),
            Vec2::new(body.x, body.y + 8.0),
            body.w,
            12.5,
            Weight::Regular,
            TEXT_DIM,
        );
        return None;
    }
    let rows: Vec<(String, String, bool)> =
        data.vehicles.iter().map(|v| (v.name.clone(), v.folder.clone(), v.from_content)).collect();
    let chosen = state.vehicle_choice;
    let list = Rect::new(body.x, body.y, body.w, (body.h - 56.0).max(60.0));
    let mut picked = None;
    ui.scroll_area("start-vehicles", list, &mut |ui, a| {
        let row_h = 40.0;
        let mut y = a.y;
        for (i, (name, folder, from_content)) in rows.iter().enumerate() {
            let row = Rect::new(a.x, y, a.w, row_h);
            if row.bottom() > a.y - row_h && row.y < a.bottom() + row_h {
                if list_row(ui, &format!("start-veh{i}"), row, name, folder, *from_content, chosen == i) {
                    picked = Some(i);
                }
            }
            y += row_h;
        }
        y - a.y
    });
    if let Some(i) = picked {
        state.vehicle_choice = i;
        ui.scroll_to("start-vehicles", i as f32 * 40.0, 40.0, list.h);
    }

    let foot = Rect::new(body.x, body.bottom() - 40.0, body.w, 40.0);
    ui.p().rect(Rect::new(foot.x, foot.y, foot.w, 1.0), EDGE);
    ui.paragraph(
        &tr("A vehicle cannot be changed from here yet: this page is where its model, its scripts and its sounds will be opened."),
        Vec2::new(foot.x, foot.y + 8.0),
        foot.w,
        11.5,
        Weight::Regular,
        TEXT_FAINT,
    );
    None
}

// ---- Testing Area -----------------------------------------------------------------------

/// What the test track is for: the two things a modder cannot check by reading a file.
fn testing_page(ui: &mut Ui, r: Rect) -> Option<Action> {
    let body = page_head(ui, r, Section::Testing, None);
    let card_h = 96.0;
    let card = |ui: &mut Ui, y: f32, title: &str, icon: &str, what: &str| {
        let c = Rect::new(body.x, y, body.w, card_h);
        ui.panel(c);
        ui.icon(icon, Vec2::new(c.x + 34.0, c.y + 34.0), 24.0, ACCENT);
        ui.text_in(&tr(title), Rect::new(c.x + 60.0, c.y + 14.0, c.w - 76.0, 24.0), 15.0, Weight::Medium, TEXT, Align::Left);
        ui.paragraph(&tr(what), Vec2::new(c.x + 60.0, c.y + 40.0), c.w - 76.0, 12.0, Weight::Regular, TEXT_DIM);
    };
    card(
        ui,
        body.y,
        "Test a vehicle",
        "directions_bus",
        "Put a bus on the track and drive it: what its scripts do, whether its dashboard and its doors work, and what it sounds like.",
    );
    card(
        ui,
        body.y + card_h + 16.0,
        "Test scenery objects",
        "construction",
        "Put an object on the track and watch it: an object with a script of its own, whose animation or variables may not do what its author meant.",
    );
    let note = Rect::new(body.x, body.y + (card_h + 16.0) * 2.0 + 12.0, body.w, 40.0);
    ui.paragraph(
        &tr("Neither is written yet. The track itself - the ground, the light and the camera - is the part the editor already has."),
        Vec2::new(note.x, note.y),
        note.w,
        12.0,
        Weight::Regular,
        TEXT_FAINT,
    );
    None
}
