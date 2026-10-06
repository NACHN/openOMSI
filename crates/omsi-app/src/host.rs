//! What a program built on this library rather than playing the game can use.
//!
//! openOMSI is three programs: `openomsi` plays a map, `openomsi-launcher` sets the game up,
//! and `openomsi-editor` edits a map. The editor is a window of its own, a process of its own
//! and a start-menu entry of its own, so someone who only wants to drive never meets it and
//! the game's own code never grows an editor's concerns - but the map is *drawn* by the same
//! code in both. That is the point of this module: an editor drawing its own idea of a tile
//! would show the modder something the game will not, which is the one thing a map editor
//! must never do.
//!
//! Nothing in here is new code. It is the same [`World`] the game loads, opened the same way,
//! and the same `build_scene` / `unload_tile` its window streams tiles with, made reachable
//! from another crate. Three things are spelled out rather than left to the caller, because
//! the game does them once at startup and forgetting any of them changes how a map looks:
//! the season's texture folder, the sun's place, and the smoke texture coronas are drawn with.
//!
//! This module is the seam, and the only place the editor touches the game. If the scenery
//! half of `omsi-app` is ever split into a crate of its own, these re-exports are what change;
//! `openomsi-editor` keeps calling the same names.

use anyhow::Result;
use glam::DVec3;
use omsi_render::{Camera, Lighting, Renderer, Scene};
use std::path::{Path, PathBuf};
use std::sync::Arc;

pub use crate::scene::{EditObject, LoadStats, TileGpu, World};
pub use omsi_editor_core::ObjectEdit;

/// The launcher's interface, for the editor to draw its own with.
///
/// The three programs show one face: the same widgets, the same palette, the same font and
/// the same icons. The editor's panels are therefore not a second toolkit - this is the
/// launcher's `ui` module made reachable, and none of it is new code. A button in the editor
/// is the launcher's `button`, with the launcher's colours and its hover animation.
///
/// The widgets are immediate-mode: every frame the caller says where a control goes and what
/// it changes, and the toolkit draws it and answers whether it was clicked.
pub mod ui {
    pub use crate::launcher::theme::*;
    pub use crate::launcher::ui::{id_of, ButtonKind, Input, Key, Ui};
    /// The same translation the widgets do: a text the editor composes itself has to be
    /// looked up where `text` would have looked it up (see `omsi_ui::tr`).
    pub use omsi_ui::tr;
}

/// The launcher's showroom, for the editor's asset list: the same picture of a model, drawn by
/// the same renderer on the same floor under the same light, with the same orbit and zoom.
///
/// It is here for the same reason `ui` is - so that the editor shows a `.sco` the way the
/// launcher shows a bus, rather than growing a second preview of its own. A `Look` with
/// `object` set (and `bus` empty) stands that scenery object on the floor instead of a vehicle;
/// `map` may be a path of its own, which is what the editor hands over for the map it has open.
pub mod showroom {
    pub use crate::launcher::showroom::{Look, Showroom};
}

/// Read the interface's language from the settings and put it in force, returning the code.
///
/// The launcher does this before its first page; the editor's panels are read in the same
/// language, so it does the same before its first frame. Without it every text is English,
/// which is what the English text of a text *is* - the tables simply add the rest.
pub fn install_ui_language() -> String {
    let code = crate::settings::Settings::load().language;
    crate::ui_language(&code);
    code
}

/// The clock an editor opens a map on: a summer morning, which is what a modder wants to see
/// by. The date also decides which chrono scenario is in force.
pub fn clock() -> omsi_sim::SimClock {
    omsi_sim::SimClock::default()
}

/// Open the map the way the game opens it, minus the traffic, the timetable and the player.
///
/// The season folder and the sun's place are set here rather than by the caller: they are
/// process-wide, the game sets them once before its first tile, and a map opened without them
/// comes out in the wrong textures and lit from the wrong direction.
pub fn open(root: &Path, map_cfg: &Path) -> Result<World> {
    let clock = clock();
    let world = World::open(root, map_cfg, clock.date_code())?;
    set_season(&world, clock.day_of_year, false);
    Ok(world)
}

/// [`open`] for a given day of the year and weather: the original game's season table, or the
/// snow textures when it is snowing.
pub fn open_on(root: &Path, map_cfg: &Path, day_of_year: i32, snow: bool) -> Result<World> {
    let mut clock = clock();
    clock.day_of_year = day_of_year;
    let world = World::open(root, map_cfg, clock.date_code())?;
    set_season(&world, day_of_year, snow);
    Ok(world)
}

/// Put the map into the season's textures: the snow set when `snow`, else whatever the map's
/// own season table says `day_of_year` falls in.
pub fn set_season(world: &World, day_of_year: i32, snow: bool) {
    let folder = if snow {
        Some("WinterSnow".to_string())
    } else {
        omsi_map::global::Season::folder(world.global.season_kind(day_of_year)).map(|f| f.to_string())
    };
    log::info!(
        "season: day {day_of_year} -> kind {} (texture folder {folder:?})",
        world.global.season_kind(day_of_year)
    );
    omsi_texture::set_season_folder(folder);
}

/// The light of a given hour on a given day, as the game computes it: the map's own time zone
/// and place decide where the sun is, so this cannot be a colour someone picked.
///
/// `fog_range` is how far the weather lets you see (m). The game passes the weather's own
/// visibility; an editor wants a long one - see `omsi_editor::view::FOG_RANGE`.
pub fn lighting(day_of_year: i32, hour: f64, fog_range: f32) -> Lighting {
    let mut clock = clock();
    clock.day_of_year = day_of_year;
    clock.time = hour * 3600.0;
    let day = omsi_sim::Daylight::compute(&clock, None);
    crate::lights::lighting_from(&day, fog_range)
}

/// The graphics device the game would pick on this machine.
pub fn instance() -> wgpu::Instance {
    crate::startup::graphics_instance()
}

/// A renderer for the editor, at the quality the settings ask for - the same options the
/// game's own window is made with, so the two draw a tile the same way.
///
/// `window` is the window it will present into, or `None` for one that draws to a picture
/// instead (see [`screenshot`]). The smoke texture and the corona root are loaded here: the
/// game does it once at startup and without them every lamp and chimney in a map goes dark.
pub fn renderer(
    instance: &mut wgpu::Instance,
    window: Option<&Arc<winit::window::Window>>,
    root: &Path,
) -> Result<Renderer> {
    let options = crate::settings::Settings::load().render_options();
    let mut renderer = match window {
        Some(w) => crate::startup::window_renderer(instance, w, options)?,
        None => pollster::block_on(Renderer::new_with(
            instance,
            None,
            Some(wgpu::TextureFormat::Rgba8UnormSrgb),
            options,
        ))?,
    };
    log::info!("drawing on {}", renderer.adapter_name);
    crate::lights::load_smoke_texture(&mut renderer, root);
    crate::lights::set_corona_root(root);
    Ok(renderer)
}

/// The tile a world position falls in.
pub fn tile_of(p: DVec3) -> (i32, i32) {
    (
        (p.x / omsi_map::tile_size()).floor() as i32,
        (p.y / omsi_map::tile_size()).floor() as i32,
    )
}

/// The map's tiles within `radius` (in tiles) of `center` - what a camera keeps loaded. Each
/// is `(x, y, file)`, and only the tiles whose files exist are there.
pub fn tiles_near(world: &World, center: DVec3, radius: i32) -> Vec<(i32, i32, PathBuf)> {
    world.select_tiles(Some(tile_of(center)), Some(radius))
}

/// Every tile the map has, for showing a whole map at once.
pub fn tiles_all(world: &World) -> Vec<(i32, i32, PathBuf)> {
    world.select_tiles(None, None)
}

/// Load, tessellate and upload `tiles`. Safe to call again and again as a camera moves: it is
/// what the game's window calls for each batch it streams in.
pub fn build(
    world: &World,
    renderer: &Renderer,
    scene: &mut Scene,
    tiles: &[(i32, i32, PathBuf)],
) -> Result<LoadStats> {
    world.build_scene(renderer, scene, tiles)
}

/// Take a tile off the GPU and out of the world's lists. Call [`release_types`] once after a
/// batch of unloads, not per tile.
pub fn unload(world: &World, renderer: &Renderer, scene: &mut Scene, key: (i32, i32)) -> bool {
    world.unload_tile(renderer, scene, key, None)
}

/// Let go of the meshes and textures no loaded tile uses any more.
pub fn release_types(world: &World) {
    world.trim_object_types();
    world.refresh_tile_lists();
}

/// Draw one frame into a picture (`RGBA8`, `w * h * 4` bytes) rather than into a window - how
/// the editor saves a view, and how its drawing is checked without a screen.
pub fn screenshot(
    renderer: &mut Renderer,
    scene: &mut Scene,
    width: u32,
    height: u32,
    camera: &Camera,
    lighting: &Lighting,
) -> Result<Vec<u8>> {
    renderer.render_to_image(scene, width, height, camera, lighting)
}

/// The object the middle of the view is on, if any - what a click chooses.
///
/// The rule is the world's own (see `World::pick_candidates`), so a click in the editor
/// chooses what the same click in the game's in-game editor chooses - and `scene` is half of
/// that rule: the map is picked where it is drawn, and what is not drawn cannot be clicked.
pub fn pick(world: &World, scene: &Scene, eye: DVec3, forward: glam::Vec3) -> Option<i64> {
    world.pick_candidates(scene, eye, forward, 150.0).first().copied()
}

/// The window icon, so the editor looks like the other two programs.
pub fn window_icon() -> Option<winit::window::Icon> {
    crate::startup::window_icon()
}

/// A camera looking at `at` from `height` metres above it, tilted down.
pub fn camera_over(at: DVec3, height: f64) -> Camera {
    Camera {
        position: at + DVec3::Z * height,
        yaw: 0.0,
        pitch: -35.0,
        roll: 0.0,
        fov_deg: 60.0,
        near: 0.5,
        far: 8000.0,
    }
}
