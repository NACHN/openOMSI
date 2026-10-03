//! openomsi-editor: the map editor.
//!
//! openOMSI is three programs. `openomsi` plays a map, `openomsi-launcher` sets the game up,
//! and this one edits a map - objects, their places and their ground, written back as the
//! files OMSI 2 reads. It is a program of its own rather than a mode of the game: someone who
//! only wants to drive never meets it, and nothing an editor needs has to be carried by
//! every player's copy of the game.
//!
//! What the three share is the *map*: the editor draws it with the same code the game does
//! (see `openomsi_game::host`), so a tile looks in the editor exactly as it will look in the game.
//! What the editor adds is `omsi-editor-core`, which knows nothing about a screen.
//!
//! # Two fronts, one core
//!
//! The window carries its own interface - the same panels, widgets and colours the launcher
//! is made of (see `ui`) - and a console inside one of them. The console and the panels, and
//! the prompt this program still falls back to when there is no window to open, all go
//! through the same `Session`. What is typed at the console and what is clicked on the map
//! are the same operation; neither can do something the others cannot, and none of them owns
//! the undo history.
//!
//! # Where an edit goes
//!
//! By default a changed tile is written as a *copy* under the game's content folder, which the
//! game reads before the installation - so a stock map is never touched. `--in-place` writes
//! the map's own files instead, which is what editing your own map means; the first change to
//! each file leaves a `<file>.openomsi-bak` snapshot beside it.

mod repl;
mod gizmo;
mod start;
mod ui;
mod view;
mod window;

use anyhow::{anyhow, bail, Context, Result};
use clap::Parser;
use omsi_editor_core::{Destination, Session};
use std::io::{BufRead, IsTerminal, Write};
use std::path::{Path, PathBuf};

#[derive(Parser, Debug)]
#[command(
    name = "openomsi-editor",
    about = "Edit an OMSI 2 map: move objects, place new ones, shape the ground",
    version,
    after_help = "A map, and every file in it, is read from the content folder before the OMSI 2\n\
                  installation: a map that came with a mod is found, and a mod's own copy of a\n\
                  tile, an object or a texture is the one that is used.\n\
                  Run it with no arguments at all and it opens the OMSI 2 folder the launcher\n\
                  knows about, asks which map to edit, and opens the window on it. The window\n\
                  carries the tools, the object list, the console and the save button.\n\
                  With no window to open - or with --no-window - commands are read from here,\n\
                  or from a file with --script. `help` lists them."
)]
struct Args {
    /// The OMSI 2 folder - the one with Omsi.exe, `maps` and `Vehicles` in it.
    #[arg(long, short = 'r')]
    root: Option<PathBuf>,

    /// The map to edit: a name under `maps/` (Grundorf) or a path to its `global.cfg`.
    /// A name is looked for in the content folder before the original installation.
    /// Left out, the maps there are offered to choose from.
    #[arg(long, short = 'm')]
    map: Option<String>,

    /// Write the map's own files rather than copies under the content folder.
    #[arg(long)]
    in_place: bool,

    /// Where a save's copies go (default: the game's own content folder).
    #[arg(long)]
    content: Option<PathBuf>,

    /// Run the commands in this file and stop. Nothing is written unless the file says `save`.
    #[arg(long, short = 's')]
    script: Option<PathBuf>,

    /// The prompt only: no window even when one could be opened.
    #[arg(long)]
    no_window: bool,

    /// Open the window even when the input is not a terminal. Nothing is read from the
    /// terminal either way: the window carries its own console.
    #[arg(long)]
    window: bool,

    /// Draw the map around this point `x y` into a PNG and stop - no window, no display.
    /// A map west or south of the origin has negative coordinates, so a leading `-` is a
    /// number here and not an option.
    #[arg(long, value_names = ["x", "y"], num_args = 2, allow_hyphen_values = true)]
    shot_at: Option<Vec<f64>>,

    /// Where `--shot-at` writes (default: `openomsi-editor.png`).
    #[arg(long, short = 'o', default_value = "openomsi-editor.png")]
    shot: PathBuf,

    /// Draw one frame of the *interface* over the map into this PNG and stop - no window.
    /// The panels as the window draws them, so a change to them can be looked at without one.
    #[arg(long, value_name = "png")]
    ui_shot: Option<PathBuf>,

    /// With `--ui-shot`: choose whatever the crosshair is on and put this tool in hand -
    /// `select`, `move` or `turn`. What the marks a tool draws look like (the bubble round the
    /// chosen object, the gizmo it is dragged by) can then be looked at without a window.
    #[arg(long, value_name = "tool")]
    ui_tool: Option<String>,

    /// With `--ui-shot`: which section of the *first* page to draw - `maps`, `vehicles` or
    /// `testing`. There are three pages before a map is open, and this is how one of them is
    /// looked at without clicking through to it.
    #[arg(long, value_name = "section")]
    ui_section: Option<String>,

    /// Look from this height above the ground for `--shot-at` (m).
    #[arg(long, default_value_t = 80.0)]
    height: f64,

    /// The hour of the day the map is lit by, 0-24 (default: the morning, 9).
    #[arg(long, default_value_t = 9.0)]
    hour: f64,
}

fn main() -> Result<()> {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("warn")).init();
    let args = Args::parse();

    let root = resolve_root(&args)?;
    // Before anything is opened: tell the readers where the content folder is. A map, and every
    // file in it, is looked for there before the original installation - exactly as the game
    // looks for it - so without this the editor would see stock maps only: a map that came with
    // a mod would not even be listed, and a mod's copy of a tile, an object or a texture would
    // be invisible under the installation's own.
    read_content(&root);
    let destination = resolve_destination(&args, &root);
    // A script and a window together are refused rather than quietly half-honoured: the one
    // reads commands from a file and stops, the other takes them from its own console, and
    // neither would notice the other had been left out.
    if args.script.is_some() && args.window {
        bail!("--script and --window together: the script would be left unread");
    }
    // a window opens unless a script was given or the prompt was asked for. Typing at a
    // terminal opens it; `--window` opens it for a session whose input is not a terminal
    let windowed = !args.no_window && args.script.is_none() && (args.window || std::io::stdin().is_terminal());
    // with nothing else asked for, someone is sitting at the prompt: offer the map list
    // rather than refusing to start
    let ask = args.script.is_none() && args.shot_at.is_none() && args.ui_shot.is_none() && std::io::stdin().is_terminal();

    // a picture of the map, drawn without a window: what the editor will draw, checked
    // without a screen (and the fastest way to see whether a map loads at all)
    if let Some(at) = &args.shot_at {
        if at.len() < 2 {
            bail!("--shot-at wants an x and a y: --shot-at 5000 6000");
        }
        let map = resolve_map(&root, args.map.as_deref(), ask)?;
        return shot(&root, &map, at[0], at[1], &args);
    }

    // the start page of the window, drawn without one: the same page, so its layout can be
    // looked at without a screen. With no `--map` it is the map list, as the window shows it
    if let Some(out) = &args.ui_shot {
        let map = match &args.map {
            Some(_) => Some(resolve_map(&root, args.map.as_deref(), ask)?),
            None => None,
        };
        return ui_shot(&root, map.as_deref(), destination, args.ui_tool.as_deref(), args.ui_section.as_deref(), out);
    }

    // which map: named, asked for here, or - when a window is going to open - asked for in
    // the window, which is the one place the question does not need a terminal
    let map = match &args.map {
        Some(_) => Some(resolve_map(&root, args.map.as_deref(), ask)?),
        None if windowed => None,
        None => Some(resolve_map(&root, None, ask)?),
    };

    if windowed {
        return window::run(&root, map, destination);
    }
    let map = map.expect("a map is resolved when no window opens");

    let mut session = Session::open(&map, destination.clone()).with_context(|| format!("opening {}", map.display()))?;
    // an intercepted Ctrl+C would leave a half-written map; the default is what we want
    banner(&session);

    match &args.script {
        Some(path) => {
            let text = std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
            run_lines(&mut session, text.lines().map(|l| l.to_string()), false)?;
        }
        None if std::io::stdin().is_terminal() => interactive(&mut session)?,
        // piped in: read the commands from stdin, as a script
        None => {
            let lines: Vec<String> = std::io::stdin().lock().lines().collect::<std::io::Result<_>>()?;
            run_lines(&mut session, lines.into_iter(), false)?;
        }
    }

    if session.doc().is_dirty() {
        println!("\n{}", unsaved_warning(&session));
    }
    Ok(())
}

/// Draw the map around `x y` into a picture: the editor's own renderer, the game's own tiles.
///
/// This is the editor's drawing with no window in front of it - the same `View`, the same
/// tiles, the same light. It is what the window shows, and the only way to look at a map
/// where there is no screen to open one on.
fn shot(root: &Path, map: &Path, x: f64, y: f64, args: &Args) -> Result<()> {
    let mut instance = openomsi_game::host::instance();
    let mut renderer = openomsi_game::host::renderer(&mut instance, None, root)?;
    let mut scene = renderer.new_scene();
    let mut view = view::View::open(root, map, &renderer, &mut scene)?;
    // stand the given height above the ground *there*, so a map with hills is looked at from
    // a height over its own surface rather than over sea level
    let z = view.world().ground_terrain(x, y).unwrap_or(0.0);
    view.look_at(glam::DVec3::new(x, y, z), args.height);
    view.stream(&renderer, &mut scene);
    renderer.prepare(&mut scene);

    let lighting = view.lighting_at(args.hour);
    let (w, h) = (1600u32, 900u32);
    let pixels = openomsi_game::host::screenshot(&mut renderer, &mut scene, w, h, view.camera(), &lighting)?;
    image::save_buffer(&args.shot, &pixels, w, h, image::ColorType::Rgba8)
        .with_context(|| format!("writing {}", args.shot.display()))?;
    println!(
        "{}: looked at ({x:.0}, {y:.0}) from {:.0} m, {:.0}:00, {} -> {}",
        view.world().global.name,
        args.height,
        args.hour,
        view.note,
        args.shot.display()
    );
    Ok(())
}

/// One frame of the editor's window drawn into a picture, without a window.
///
/// With a map: the two passes the window makes - the map, then the panels with no clear over
/// it. Without one: the start page, which is the page the window opens on. The same panels at
/// the same size either way, so the layout can be looked at where there is no screen to open
/// a window on - and so a change to it can be checked without one.
fn ui_shot(
    root: &Path,
    map: Option<&Path>,
    destination: Destination,
    mark: Option<&str>,
    section: Option<&str>,
    out: &Path,
) -> Result<()> {
    openomsi_game::host::install_ui_language();
    let mut instance = openomsi_game::host::instance();
    let mut renderer = openomsi_game::host::renderer(&mut instance, None, root)?;
    let mut panels = ui::Panels::new();
    let (w, h) = (1600u32, 900u32);
    let dt = 1.0 / 60.0;

    // what the window would be showing: a map, or the page that asks which one
    let mut session = match map {
        Some(m) => Some(Session::open(m, destination.clone())?),
        None => None,
    };
    let opened = match &session {
        Some(_) => {
            let mut scene = renderer.new_scene();
            let view = view::View::open(root, map.expect("a map was opened"), &renderer, &mut scene)?;
            renderer.prepare(&mut scene);
            Some((scene, view))
        }
        None => None,
    };

    if session.is_none() {
        panels.set_maps(map_entries(root));
        panels.set_vehicles(vehicle_entries());
        // which of the three pages to draw (the map editor's own is what it opens on)
        if let Some(name) = section {
            match start::Section::named(name) {
                Some(s) => panels.show_section(s),
                None => bail!("--ui-section {name}: expected maps, vehicles or testing"),
            }
        }
    }
    // what the window would tell them: the panels decide the layout, the caller the size
    let frame = match (&mut session, opened) {
        (Some(session), Some((mut scene, view))) => {
            // a tool with something chosen: what the marks look like - the bubble and the
            // gizmo - needs an object, and the crosshair's own is the one the window would
            // have chosen from where it stands
            if let Some(name) = mark {
                let id = openomsi_game::host::pick(view.world(), view.camera().position, view.camera().forward());
                session.select(id);
                panels.tool = match name {
                    "move" => ui::Tool::Move,
                    "turn" => ui::Tool::Turn,
                    "place" => ui::Tool::Place,
                    "delete" => ui::Tool::Delete,
                    _ => ui::Tool::Select,
                };
                println!("{name}: object {id:?} chosen");
            }
            let info = ui::Info { fps: 60.0, loaded_tiles: view.loaded_count(), aim: view.aim(), holding: None };
            let frame = panels.build(session, &view, &info, &renderer, (w, h), 1.0, dt);
            // the marks, drawn on the map as the window draws them - before the frame that
            // draws the map, which is where the window does it
            let shown = view::Shown::default();
            let chosen = session.selection();
            let drawn = chosen.as_ref().and_then(|s| shown.where_drawn(view.world(), s.id));
            let bubble = chosen.as_ref().and_then(|s| shown.bubble(view.world(), &scene, s.id));
            let (show, _) = gizmo::show_for(panels.tool, None, chosen, drawn, bubble, view.camera(), h as f32);
            let mut marks = gizmo::Marks::default();
            marks.draw(&renderer, &mut scene, &show);
            // the map goes under the panels, in the same target, before they are drawn over it
            let target = renderer.device.create_texture(&wgpu::TextureDescriptor {
                label: Some("editor ui shot"),
                size: wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: renderer.format(),
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
                view_formats: &[],
            });
            let tv = target.create_view(&Default::default());
            renderer.render(&mut scene, &tv, w, h, view.camera(), &view.lighting());
            return compose(&mut panels, &frame, &renderer, &target, &tv, (w, h), out);
        }
        _ => {
            let content = omsi_launcher_lib::player_content_dir();
            let start = ui::Start { root, content: content.as_deref(), destination: &destination };
            panels.build_start(&start, &renderer, (w, h), 1.0, dt)
        }
    };

    // the start page draws its own background, but a texture's content is undefined until
    // something writes it: the whole target is cleared first, so nothing of it shows through
    let target = renderer.device.create_texture(&wgpu::TextureDescriptor {
        label: Some("editor start page shot"),
        size: wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: renderer.format(),
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    });
    let tv = target.create_view(&Default::default());
    {
        let mut enc = renderer.device.create_command_encoder(&Default::default());
        enc.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("editor start page shot"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: &tv,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations { load: wgpu::LoadOp::Clear(wgpu::Color::BLACK), store: wgpu::StoreOp::Store },
            })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });
        renderer.queue.submit([enc.finish()]);
    }
    compose(&mut panels, &frame, &renderer, &target, &tv, (w, h), out)
}

/// Draw the panels over what is already in `target` and write the result to `out`.
fn compose(
    panels: &mut ui::Panels,
    frame: &ui::Frame,
    renderer: &omsi_render::Renderer,
    target: &wgpu::Texture,
    tv: &wgpu::TextureView,
    (w, h): (u32, u32),
    out: &Path,
) -> Result<()> {
    let mut enc = renderer.device.create_command_encoder(&Default::default());
    panels.render(frame, renderer, &mut enc, tv, (w, h));
    // the rows of a texture copy are padded to 256 bytes, so the picture is read back the
    // long way round rather than straight into the file
    let bpr = (w * 4).div_ceil(256) * 256;
    let buf = renderer.device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("ui shot readback"),
        size: (bpr * h) as u64,
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    enc.copy_texture_to_buffer(
        wgpu::TexelCopyTextureInfo { texture: target, mip_level: 0, origin: wgpu::Origin3d::ZERO, aspect: wgpu::TextureAspect::All },
        wgpu::TexelCopyBufferInfo {
            buffer: &buf,
            layout: wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(bpr), rows_per_image: Some(h) },
        },
        wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
    );
    let index = renderer.queue.submit([enc.finish()]);
    let slice = buf.slice(..);
    let (tx, rx) = std::sync::mpsc::channel();
    slice.map_async(wgpu::MapMode::Read, move |r| {
        let _ = tx.send(r);
    });
    renderer
        .device
        .poll(wgpu::PollType::Wait { submission_index: Some(index), timeout: None })
        .map_err(|e| anyhow!("poll: {e:?}"))?;
    rx.recv().context("map")?.map_err(|e| anyhow!("map: {e:?}"))?;
    let data = slice.get_mapped_range();
    let mut pixels = Vec::with_capacity((w * h * 4) as usize);
    for row in 0..h {
        let start = (row * bpr) as usize;
        pixels.extend_from_slice(&data[start..start + (w * 4) as usize]);
    }
    drop(data);
    buf.unmap();
    if matches!(renderer.format(), wgpu::TextureFormat::Bgra8Unorm | wgpu::TextureFormat::Bgra8UnormSrgb) {
        for px in pixels.chunks_exact_mut(4) {
            px.swap(0, 2);
        }
    }
    image::save_buffer(out, &pixels, w, h, image::ColorType::Rgba8).with_context(|| format!("writing {}", out.display()))?;
    println!("the editor's window, {w}x{h} -> {}", out.display());
    Ok(())
}

/// The maps the editor can open, with where each copy of one was found.
///
/// A name is resolved through the content folder first and the installation second, exactly
/// as the game resolves it, so the copy the list names is the copy that opens - and which of
/// the two it came from is what a save will touch.
fn map_entries(root: &Path) -> Vec<ui::MapEntry> {
    let content = omsi_launcher_lib::player_content_dir();
    available_maps(root)
        .into_iter()
        .map(|name| {
            let cfg = map_path(root, &name);
            let from_content = content.as_ref().is_some_and(|c| under(&cfg, c));
            ui::MapEntry { name, cfg, from_content }
        })
        .collect()
}

/// The vehicles the vehicle page lists: every `.bus` and `.ovh` under `Vehicles`, read from
/// the content folder first and the installation second - the same two folders, and the same
/// order, that a map is found in.
///
/// A name that is in both folders is listed once, and which copy it is is the content
/// folder's: it is what the game itself would load. A folder holding both `X.bus` and `X.ovh`
/// is one vehicle, named by the `.bus` - the file a bus is started from - because the pair is
/// how a bus is written, not two buses.
fn vehicle_entries() -> Vec<ui::VehicleEntry> {
    let content = omsi_launcher_lib::player_content_dir();
    let mut folders: Vec<String> = omsi_cfg::read_dir_merged("Vehicles")
        .into_iter()
        .filter_map(|d| d.file_name().map(|n| n.to_string_lossy().to_string()))
        .collect();
    folders.sort();
    let mut out: Vec<ui::VehicleEntry> = Vec::new();
    for folder in folders {
        // by name, with the `.bus` winning: a folder is read whole before its names are
        // sorted, so which of the two files turned up first cannot decide it
        let mut named: std::collections::HashMap<String, (std::path::PathBuf, bool)> = std::collections::HashMap::new();
        for f in omsi_cfg::read_dir_merged(&format!("Vehicles/{folder}")) {
            let extension = f.extension().map(|e| e.to_string_lossy().to_ascii_lowercase());
            let bus = match extension.as_deref() {
                Some("bus") => true,
                Some("ovh") => false,
                _ => continue,
            };
            let stem = f.file_stem().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
            if stem.is_empty() {
                continue;
            }
            if !named.get(&stem).is_some_and(|(_, already_a_bus)| *already_a_bus) {
                named.insert(stem, (f, bus));
            }
        }
        let mut names: Vec<(String, std::path::PathBuf)> = named.into_iter().map(|(k, (p, _))| (k, p)).collect();
        names.sort();
        for (name, path) in names {
            let from_content = content.as_ref().is_some_and(|c| under(&path, c));
            out.push(ui::VehicleEntry { name, folder: folder.clone(), from_content });
        }
    }
    out
}

/// Whether `path` is inside `dir`, by the text of the two - the paths here come from the same
/// resolver, so they are spelled the same way, and a map may live in an archive where nothing
/// can be canonicalised.
fn under(path: &Path, dir: &Path) -> bool {
    let norm = |p: &Path| p.to_string_lossy().replace('\\', "/").to_lowercase();
    norm(path).starts_with(&norm(dir))
}

/// The OMSI 2 folder: the one given, else the one the launcher already knows.
///
/// The launcher is where a person tells openOMSI where OMSI 2 is, and it remembers the answer
/// (`~/.openomsi/launcher.json`, `~/.openomsi-root`, `$OMSI_ROOT`). Reading it back rather
/// than searching again means the editor edits the map the game would have started - and that
/// a machine whose OMSI 2 is not in a folder the search knows still opens.
fn resolve_root(args: &Args) -> Result<PathBuf> {
    if let Some(r) = &args.root {
        if !r.join("maps").is_dir() {
            bail!("{}: no `maps` folder in it - is this the OMSI 2 folder?", r.display());
        }
        return Ok(r.clone());
    }
    let mut hints: Vec<PathBuf> = Vec::new();
    let configured = omsi_launcher_lib::load_config().root;
    if !configured.trim().is_empty() {
        hints.push(PathBuf::from(configured.trim()));
    }
    if let Ok(cwd) = std::env::current_dir() {
        hints.push(cwd);
    }
    // (`content_dir` deliberately not consulted here: it answers for *this* program's folder,
    // and asking it would register a build folder as the content root before `read_content`
    // gets to name the right one - `register_roots` only ever takes the first)
    match omsi_cfg::find_original_install(&hints) {
        Some(r) => {
            println!("OMSI 2 found: {}", r.display());
            Ok(r)
        }
        None => bail!("no OMSI 2 installation found - pass --root <folder>"),
    }
}

/// Tell the readers where the content folder is, before anything is opened.
///
/// The folder is the player's: the one that belongs to the game the launcher starts, see
/// `player_content_dir`. It is deliberately *not* worked out from this program's own folder -
/// the editor ships with no game, so its own folder is a build folder - and it is registered
/// before the map is looked for, because registering it is what makes every read content-first.
fn read_content(root: &Path) {
    match omsi_launcher_lib::player_content_dir() {
        Some(c) => println!("reading: {} first, then {}", c.display(), root.display()),
        None => println!("reading: {} (no content folder is known)", root.display()),
    }
    // the folder this run was pointed at, so that it counts even when the launcher's settings
    // name another one. It goes last: the content folder wins.
    omsi_cfg::add_content_root(root.to_path_buf());
}

/// The map's `global.cfg`, resolved the way the game resolves the one the launcher hands it -
/// through the content folder first, so a map that came with a mod is the map that opens.
///
/// With `ask`, a missing `--map` puts the maps there are to a question instead of ending the
/// run.
fn resolve_map(root: &Path, map: Option<&str>, ask: bool) -> Result<PathBuf> {
    let Some(m) = map else {
        let maps = available_maps(root);
        if ask {
            return choose_map(root, &maps);
        }
        if maps.is_empty() {
            bail!("no map found: nothing with a `global.cfg` in {} or in the content folder", root.display());
        }
        bail!("which map? --map <name>\n  {}", maps.join("\n  "));
    };
    let path = map_path(root, m);
    if !path.is_file() {
        bail!("{}: no map there", path.display());
    }
    Ok(path)
}

/// Where the map called `m` is: a path when one was given, else a name under `maps/`. A name
/// is relative to a content root, and `resolve_path` reads those in order - the content folder
/// before the original installation.
fn map_path(root: &Path, m: &str) -> PathBuf {
    let given = Path::new(m);
    if given.is_absolute() {
        return if given.is_dir() { given.join("global.cfg") } else { given.to_path_buf() };
    }
    let rel = if m.to_ascii_lowercase().ends_with(".cfg") {
        m.to_string()
    } else {
        format!("maps/{m}/global.cfg")
    };
    omsi_cfg::resolve_path(root, &rel)
}

/// Ask which map to open, out of the ones there are. A number, a name, or just Enter for the
/// first - and a lone map needs no answer.
fn choose_map(root: &Path, maps: &[String]) -> Result<PathBuf> {
    if maps.is_empty() {
        bail!("no map found: nothing with a `global.cfg` in {} or in the content folder", root.display());
    }
    println!("\nWhich map?\n");
    let width = maps.iter().map(|m| m.len()).max().unwrap_or(0);
    for (i, m) in maps.iter().enumerate() {
        println!("  {:>2}  {m:<width$}", i + 1);
    }
    let mut picked = maps[0].clone();
    if maps.len() > 1 {
        print!("\nnumber or name [1]: ");
        std::io::stdout().flush()?;
        let mut line = String::new();
        if std::io::stdin().read_line(&mut line)? == 0 {
            bail!("no map chosen");
        }
        picked = pick_map(maps, line.trim())?.to_string();
    }
    println!("\nOpening {picked}.");
    Ok(map_path(root, &picked))
}

/// Which map a line of input chooses: its number in the list, or its name, or the first one
/// for an empty answer. What was typed is the only thing that decides, so this can be checked
/// without a terminal.
fn pick_map<'a>(maps: &'a [String], answer: &str) -> Result<&'a str> {
    if answer.is_empty() {
        return Ok(maps.first().map(String::as_str).unwrap_or_default());
    }
    if let Ok(n) = answer.parse::<usize>() {
        // 1 is the first: a list that starts at 0 would surprise, a list that starts at 1
        // is how every menu reads
        return maps.get(n.wrapping_sub(1)).map(String::as_str).ok_or_else(|| anyhow!("there is no map {n} in the list"));
    }
    maps.iter()
        .find(|m| m.eq_ignore_ascii_case(answer))
        .map(String::as_str)
        .ok_or_else(|| anyhow!("no map named `{answer}` here"))
}

/// The maps there are to edit, by name.
///
/// The names come from every place a map can be - the content folder, the archives kept in it,
/// the installation - which is what the game and the launcher see. A name is kept only when
/// some place has its `global.cfg`: a mod that patches part of a stock map (its timetable,
/// say) leaves a `maps/Grundorf` without a `global.cfg`, and that must not take the map off
/// the list. Reading the list back through [`map_path`] means the name offered and the map
/// opened are always the same copy.
fn available_maps(root: &Path) -> Vec<String> {
    let mut names: Vec<String> = omsi_cfg::read_dir_merged("maps")
        .into_iter()
        .filter_map(|d| d.file_name().map(|n| n.to_string_lossy().to_string()))
        .collect();
    names.sort();
    names.retain(|n| omsi_cfg::vfs::is_file(&map_path(root, n)));
    names
}

fn resolve_destination(args: &Args, root: &Path) -> Destination {
    if args.in_place {
        return Destination::InPlace;
    }
    // the folder the map was read from: an edit belongs where the game will look for it
    let root_dir = args.content.clone().or_else(omsi_launcher_lib::player_content_dir).unwrap_or_else(|| root.join("content"));
    Destination::Content(root_dir)
}

fn banner(session: &Session) {
    let d = session.doc();
    println!("openOMSI editor - {}", d.global().name);
    println!("  map       {}", d.map_cfg().display());
    println!("  objects   {} in {} tiles", d.object_count(), d.tile_count());
    println!(
        "  writing   {}",
        match d.destination() {
            Destination::Content(root) => format!("copies under {}", root.display()),
            Destination::InPlace => "the map's own files, with a .openomsi-bak snapshot each".into(),
        }
    );
    println!("  commands  `help`, and `save` to write");
    println!();
}

/// The line printed before each prompt: where we are and what is selected.
fn status_line(session: &Session) -> String {
    let d = session.doc();
    let selected = if session.selected_id().is_some() {
        format!("   |   {}", session.selection_summary())
    } else {
        String::new()
    };
    format!(
        "{}{} {} objects, {} tiles{}",
        d.global().name,
        if d.is_dirty() { "*" } else { "" },
        d.object_count(),
        d.tile_count(),
        selected
    )
}

fn interactive(session: &mut Session) -> Result<()> {
    let stdin = std::io::stdin();
    let mut line = String::new();
    loop {
        println!("{}", status_line(session));
        print!("> ");
        std::io::stdout().flush()?;
        line.clear();
        if stdin.read_line(&mut line)? == 0 {
            println!();
            return Ok(());
        }
        match repl::run_line(session, &line) {
            Ok(o) => {
                if !o.message.is_empty() {
                    println!("{}", o.message);
                }
                if o.flow == repl::Flow::Quit {
                    return Ok(());
                }
            }
            Err(e) => println!("? {e}"),
        }
        println!();
    }
}

/// Run lines, one command each. A command that fails says so and the next one still runs: a
/// script's mistakes should not throw away the work around them.
fn run_lines(session: &mut Session, lines: impl Iterator<Item = String>, echo: bool) -> Result<()> {
    for (n, line) in lines.enumerate() {
        if echo {
            println!("> {line}");
        }
        match repl::run_line(session, &line) {
            Ok(o) => {
                if !o.message.is_empty() {
                    println!("{}", o.message);
                }
                if o.flow == repl::Flow::Quit {
                    return Ok(());
                }
            }
            Err(e) => println!("line {}: {e}", n + 1),
        }
    }
    Ok(())
}

fn unsaved_warning(session: &Session) -> String {
    format!(
        "{} tile(s) changed and not written - `save` writes them",
        session.doc().dirty_tiles().len()
    )
}

#[cfg(test)]
mod tests {
    use super::pick_map;

    fn maps() -> Vec<String> {
        vec!["Berlin-Spandau".into(), "Grundorf".into(), "Tiny".into()]
    }

    #[test]
    fn a_number_picks_from_one_and_enter_picks_the_first() {
        let m = maps();
        assert_eq!(pick_map(&m, "1").unwrap(), "Berlin-Spandau");
        assert_eq!(pick_map(&m, "2").unwrap(), "Grundorf");
        assert_eq!(pick_map(&m, "3").unwrap(), "Tiny");
        assert_eq!(pick_map(&m, "").unwrap(), "Berlin-Spandau");
    }

    #[test]
    fn a_name_picks_whatever_it_is_called() {
        let m = maps();
        assert_eq!(pick_map(&m, "Tiny").unwrap(), "Tiny");
        assert_eq!(pick_map(&m, "grundorf").unwrap(), "Grundorf");
    }

    #[test]
    fn a_choice_that_is_not_in_the_list_says_so() {
        let m = maps();
        // 0 is below the list rather than "one before the first"
        assert!(pick_map(&m, "0").is_err());
        assert!(pick_map(&m, "4").is_err());
        assert!(pick_map(&m, "Spandau").is_err());
    }
}
