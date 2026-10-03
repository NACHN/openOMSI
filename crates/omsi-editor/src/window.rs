//! The editor's window: the map drawn as the game draws it, with the panels over it.
//!
//! There is no terminal beside this window. The panels *are* the interface - the launcher's
//! own widgets, in the launcher's own colours - and the commands are still there, in the
//! console panel at the bottom left, because they were never the problem: a console was.
//! Someone who knows the commands types them; someone who does not clicks the rail.
//!
//! The window opens on the start page - which map? - because that is the one thing the
//! editor cannot work out for itself, and asking it at a prompt is what a double click used
//! to get. Once a map is open the start page is replaced by the editing panels over it.
//!
//! Both go through the same `Session`, so a click cannot do something a line cannot, and
//! neither owns the undo history. That is what the in-game editor got wrong: its keystrokes
//! reached into the world directly, so there was no way to say what an operation *was*, and
//! no undo that could be trusted.
//!
//! So this file decides *when* a thing is asked for, never *what* it is. It reads keys, moves
//! a camera, streams tiles, and hands the panel's clicks to the core.

use crate::gizmo::{self, Gizmo, Handle, Kind, Marks, Screen};
use crate::ui::{Info, Panels, Start, Tool};
use crate::view::{Shown, View};
use anyhow::Result;
use glam::{DVec3, Vec2};
use openomsi_game::host;
use omsi_editor_core::{Destination, ObjectEdit, Selection, Session};
use omsi_render::{Renderer, Scene, SurfaceState};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;
use winit::application::ApplicationHandler;
use winit::event::{ElementState, MouseButton, MouseScrollDelta, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::keyboard::{KeyCode, ModifiersState, PhysicalKey};
use winit::window::{CursorIcon, Window, WindowId};

/// The key name the widgets use, so the window and the toolkit agree about Tab and Enter.
use openomsi_game::host::ui::Key as PanelKey;

/// A drag in progress on a gizmo handle.
///
/// The gizmo is kept as it was when the handle was taken hold of, and the object as it stood:
/// the axes are as long as what fits the screen (see `Gizmo::size_for`), so a gizmo recomputed
/// every frame would change scale under the hand - and the drag would slow down or speed up as
/// the object came towards the camera.
struct Drag {
    handle: Handle,
    gizmo: Gizmo,
    /// Where the pointer was when the handle was taken.
    grab: Vec2,
    /// The chosen object as it stood, and what had been done to it.
    start: Selection,
    /// Where on the object's own plane the pointer was - what a turn is measured from, so the
    /// object turns by the angle the pointer moved rather than jumping to face it.
    from: DVec3,
}

/// The editor program: the editing session, the map drawn, and the panels over it.
///
/// The session is an `Option` because the window opens before a map does: with none, the
/// window is the start page, and everything that needs a map is skipped until one is chosen.
struct Editor {
    root: PathBuf,
    /// The game's content folder, for the start page's "reading" line.
    content: Option<PathBuf>,
    /// Where a save goes, decided before the session is opened (and used again when the write
    /// destination is switched - see [`Editor::reopen`]).
    destination: Destination,
    session: Option<Session>,
    /// The map's own `global.cfg`, once one is open.
    map_cfg: Option<PathBuf>,
    panels: Panels,
    view: Option<View>,
    /// What the session has already changed on the screen.
    shown: Shown,
    /// The bubble round the chosen object and the gizmo it is dragged by.
    marks: Marks,
    /// The gizmo as it stands this frame: where it is, and how long its axes are. Kept from
    /// the frame just drawn, because a handle has to be picked where it was drawn.
    gizmo_now: Option<Gizmo>,
    /// Which gizmo that is, for the same reason: the arrows and the ring come out the same
    /// length, so a pick has to be told which of the two is on the screen.
    kind_now: Kind,
    /// The handle under the pointer, as of the last frame - and what a press would take hold
    /// of. Held across frames because the handle under the pointer is drawn longer, and the
    /// pick has to look at the length that is on the screen.
    hot: Option<Handle>,
    /// A drag in progress on a handle.
    drag: Option<Drag>,
    /// The left button, as the window sees it. The panels use the frame's own copy of the
    /// input and throw it away when the frame is over, so "is it still held" has to be
    /// remembered here.
    left_down: bool,
    window: Option<Arc<Window>>,
    surface: Option<SurfaceState<'static>>,
    renderer: Option<Renderer>,
    scene: Option<Scene>,
    instance: wgpu::Instance,
    /// Keys held down, so a camera keeps moving while one is.
    held: hashbrown::HashSet<KeyCode>,
    modifiers: ModifiersState,
    /// Right mouse held: the view turns with the pointer.
    looking: bool,
    last: Instant,
    /// The pointer in physical pixels, which is what a handle is picked in (the gizmo is
    /// projected in the pixels the frame is drawn in).
    cursor: (f64, f64),
    fps: f32,
    /// Where the middle of the view meets the ground, as of the frame being drawn.
    aim: Option<DVec3>,
    /// Set once the window has been asked to close.
    closing: bool,
}

/// Open the window - on `map` when one was named, on the start page when none was.
pub fn run(root: &Path, map_cfg: Option<PathBuf>, destination: Destination) -> Result<()> {
    // the panels are read in the language the launcher is set to, as its own pages are
    let language = host::install_ui_language();
    log::info!("the editor's panels are in `{language}`");
    let session = match &map_cfg {
        Some(p) => Some(Session::open(p, destination.clone())?),
        None => None,
    };
    let event_loop = EventLoop::new()?;
    event_loop.set_control_flow(ControlFlow::Poll);
    let mut editor = Editor {
        root: root.to_path_buf(),
        content: omsi_launcher_lib::player_content_dir(),
        destination,
        session,
        map_cfg,
        panels: Panels::new(),
        view: None,
        shown: Shown::default(),
        marks: Marks::default(),
        gizmo_now: None,
        kind_now: Kind::None,
        hot: None,
        drag: None,
        left_down: false,
        window: None,
        surface: None,
        renderer: None,
        scene: None,
        instance: host::instance(),
        held: hashbrown::HashSet::new(),
        modifiers: ModifiersState::empty(),
        looking: false,
        last: Instant::now(),
        cursor: (0.0, 0.0),
        fps: 0.0,
        aim: None,
        closing: false,
    };
    // the page's list, read before the window is opened so the first frame has it
    if editor.session.is_none() {
        editor.panels.set_maps(crate::map_entries(root));
        editor.panels.set_vehicles(crate::vehicle_entries());
    }
    event_loop.run_app(&mut editor)?;
    // the window is gone, but an unwritten change is not: this is the last place to say so
    if let Some(s) = editor.session.as_ref() {
        if s.doc().is_dirty() {
            println!(
                "\n{} tile(s) changed and not written - open the map again and `save` writes them",
                s.doc().dirty_tiles().len()
            );
        }
    }
    Ok(())
}

impl Editor {
    /// Create the window, the renderer and the surface - once, on the first `resumed`.
    fn open_window(&mut self, event_loop: &ActiveEventLoop) {
        let attrs = Window::default_attributes()
            .with_title(self.title())
            .with_inner_size(winit::dpi::LogicalSize::new(1600u32, 900u32))
            .with_window_icon(host::window_icon());
        let window = match event_loop.create_window(attrs) {
            Ok(w) => Arc::new(w),
            Err(e) => {
                log::error!("the editor's window could not be opened: {e}");
                event_loop.exit();
                return;
            }
        };
        let renderer = match host::renderer(&mut self.instance, Some(&window), &self.root) {
            Ok(r) => r,
            Err(e) => {
                log::error!("the editor cannot draw on this computer: {e:#}");
                event_loop.exit();
                return;
            }
        };
        let size = window.inner_size();
        let surface = match SurfaceState::new_with(&self.instance, window.clone(), &renderer, size.width.max(1), size.height.max(1), true) {
            Ok(s) => s,
            Err(e) => {
                log::error!("the editor's window cannot be drawn into: {e:#}");
                event_loop.exit();
                return;
            }
        };
        self.renderer = Some(renderer);
        self.surface = Some(surface);
        self.window = Some(window.clone());
        // a map named on the command line is opened here, where there is a renderer to draw
        // it with; the start page is left standing otherwise
        if let Some(map) = self.map_cfg.clone() {
            self.open_map(&map);
        } else {
            self.panels.say(if self.panels.maps.is_empty() {
                "No map was found to edit."
            } else {
                "Choose a map to edit."
            });
        }
        window.request_redraw();
        self.last = Instant::now();
    }

    /// The window's own title: what is open, or that nothing is yet.
    fn title(&self) -> String {
        if let Some(s) = self.session.as_ref() {
            format!("openOMSI editor - {}", s.doc().global().name)
        } else {
            "openOMSI editor".to_string()
        }
    }

    /// Open a map: the session, the scene and the view that draws it. The start page's
    /// answer, and the same call the command line's `--map` goes through.
    fn open_map(&mut self, map_cfg: &Path) {
        let opened = (|| -> Result<(Session, String)> {
            let session = Session::open(map_cfg, self.destination.clone())?;
            let name = session.doc().global().name.clone();
            Ok((session, name))
        })();
        let (session, name) = match opened {
            Ok(v) => v,
            Err(e) => {
                self.panels.say(format!("{}: {e:#}", host::ui::tr("could not open the map")));
                return;
            }
        };
        // the tiles are read with the game's own code, and a failure here is worth saying
        // rather than leaving a window with nothing in it
        let built = {
            let Some(renderer) = self.renderer.as_ref() else { return };
            let mut scene = renderer.new_scene();
            match View::open(&self.root, map_cfg, renderer, &mut scene) {
                Ok(view) => {
                    renderer.prepare(&mut scene);
                    Ok((scene, view))
                }
                Err(e) => Err(e),
            }
        };
        let (scene, view) = match built {
            Ok(v) => v,
            Err(e) => {
                self.panels.say(format!("{e:#}"));
                return;
            }
        };
        self.panels
            .say(format!("{name}: {} {} / {} {}", session.doc().object_count(), host::ui::tr("objects"), session.doc().tile_count(), host::ui::tr("tiles")));
        self.panels.say(match session.doc().destination() {
            Destination::Content(p) => format!("{}\n{}", host::ui::tr("A save writes copies under"), p.display()),
            Destination::InPlace => host::ui::tr("A save writes the map's own files, with a .openomsi-bak snapshot each").to_string(),
        });
        self.session = Some(session);
        self.map_cfg = Some(map_cfg.to_path_buf());
        self.scene = Some(scene);
        self.view = Some(view);
        self.shown = Shown::default();
        // the marks were built against the scene that has just been thrown away
        self.marks.reset();
        self.gizmo_now = None;
        self.hot = None;
        self.drag = None;
        self.gizmo_now = None;
        self.kind_now = Kind::None;
        self.aim = None;
        if let Some(w) = self.window.as_ref() {
            w.set_title(&self.title());
        }
    }

    /// One frame: the map and the panels over it, or the start page while there is no map.
    fn draw(&mut self, dt: f32) {
        let Some(window) = self.window.clone() else { return };
        let (Some(surface), Some(renderer)) = (self.surface.as_ref(), self.renderer.as_mut()) else { return };
        let (wgpu::CurrentSurfaceTexture::Success(frame) | wgpu::CurrentSurfaceTexture::Suboptimal(frame)) =
            surface.surface.get_current_texture()
        else {
            return;
        };
        let size = (surface.config.width, surface.config.height);
        let scale = window.scale_factor() as f32;
        let target = frame.texture.create_view(&Default::default());
        // read before the frame's own borrows: what is hovered and what is dragged are what
        // the marks are drawn from
        let (hot, tool) = (self.hot, self.panels.tool);
        let dragging = self.drag.as_ref().map(|d| d.handle);
        self.gizmo_now = None;

        let ui_frame = match (self.session.as_mut(), self.scene.as_mut(), self.view.as_mut()) {
            (Some(session), Some(scene), Some(view)) => {
                // the tiles follow the camera; the frame after a tile arrives is the one that
                // shows it
                let stats = view.stream(renderer, scene);
                if stats.tiles > 0 {
                    renderer.prepare(scene);
                }
                // and what the session has changed is put on the screen before it is drawn. A
                // click shows itself a frame late, which is the price of not holding a second
                // copy of the edit logic here - and the right price.
                view.sync(session, &mut self.shown, renderer, scene);

                // the bubble round the chosen object, and the gizmo its tool puts on it - put
                // on the map before the frame that draws them
                let chosen = session.selection();
                let drawn = chosen.as_ref().and_then(|s| self.shown.where_drawn(view.world(), s.id));
                let bubble = chosen.as_ref().and_then(|s| self.shown.bubble(view.world(), scene, s.id));
                let (show, gizmo) = gizmo::show_for(tool, dragging.or(hot), chosen, drawn, bubble, view.camera(), size.1 as f32);
                self.marks.draw(renderer, scene, &show);
                // where the gizmo is, for the frame that has to pick a handle on it: the one
                // that was just drawn rather than one worked out again later. While a drag is
                // in hand the frozen one is the one in use.
                self.kind_now = show.kind;
                self.gizmo_now = gizmo.filter(|_| self.drag.is_none());

                let camera = view.camera().clone();
                let lighting = view.lighting();
                renderer.render(scene, &target, size.0, size.1, &camera, &lighting);

                let holding = dragging.or(hot).map(|h| h.name());
                let info = Info { fps: self.fps, loaded_tiles: view.loaded_count(), aim: self.aim, holding };
                self.panels.build(session, view, &info, renderer, size, scale, dt)
            }
            // nothing is open yet: the page that asks which map
            _ => {
                let start = Start { root: &self.root, content: self.content.as_deref(), destination: &self.destination };
                self.panels.build_start(&start, renderer, size, scale, dt)
            }
        };

        // the panels, in a second pass into the very target the map went into: no clear, so
        // the map stands under them (see `Panels::render`)
        let mut encoder = renderer.device.create_command_encoder(&Default::default());
        self.panels.render(&ui_frame, renderer, &mut encoder, &target, size);
        renderer.queue.submit(Some(encoder.finish()));

        // a hand over a button, an I-beam in a field, a crosshair over the map. While the
        // right button is held the pointer goes altogether - the view turns with the mouse,
        // and a pointer stuck in the middle of the turn is what the game hides too.
        let over_panels = self.panels.cursor();
        window.set_cursor(over_panels.unwrap_or(if self.looking { CursorIcon::Default } else { CursorIcon::Crosshair }));
        window.set_cursor_visible(!self.looking);
        window.pre_present_notify();
        frame.present();
    }

    /// Move the camera by however long the last frame took. Not while a field has the keys:
    /// someone typing `mv 2 0 0` into the console is not flying south.
    fn tick(&mut self, dt: f64) {
        if self.panels.typing() {
            return;
        }
        let Some(view) = self.view.as_mut() else { return };
        let down = |k: KeyCode| self.held.contains(&k);
        let forward = (down(KeyCode::KeyW) as i32 - down(KeyCode::KeyS) as i32) as f64;
        let right = (down(KeyCode::KeyD) as i32 - down(KeyCode::KeyA) as i32) as f64;
        let up = (down(KeyCode::KeyE) as i32 - down(KeyCode::KeyQ) as i32) as f64;
        view.fly(forward, right, up, dt);
    }

    /// The arrow keys: nudge what is chosen along the ground, or turn it with Shift.
    ///
    /// Along the camera's own axes rather than the map's: "left" is what left looks like from
    /// where the map is being looked at, which is the only left the person at the screen has.
    fn nudge(&mut self, code: KeyCode) {
        let Some(view) = self.view.as_ref() else { return };
        if self.session.as_ref().and_then(|s| s.selected_id()).is_none() {
            return;
        }
        // flattened onto the ground: an arrow moves along the map, not into it
        let (f, r) = (flat(view.camera().forward().as_dvec3()), flat(view.camera().right().as_dvec3()));
        let (step, turn) = (self.panels.step(), self.panels.turn_step());
        let shift = self.modifiers.shift_key();
        match (code, shift) {
            (KeyCode::ArrowLeft, false) => self.act(move |s| s.move_selected(r * -step)),
            (KeyCode::ArrowRight, false) => self.act(move |s| s.move_selected(r * step)),
            (KeyCode::ArrowUp, false) => self.act(move |s| s.move_selected(f * step)),
            (KeyCode::ArrowDown, false) => self.act(move |s| s.move_selected(f * -step)),
            (KeyCode::PageUp, _) => self.act(move |s| s.move_selected(DVec3::Z * step)),
            (KeyCode::PageDown, _) => self.act(move |s| s.move_selected(DVec3::Z * -step)),
            (KeyCode::ArrowLeft, true) => self.act(move |s| s.turn_selected(-turn)),
            (KeyCode::ArrowRight, true) => self.act(move |s| s.turn_selected(turn)),
            _ => {}
        }
    }

    /// A left click the panels did not take: the tool in hand, aimed at the crosshair.
    ///
    /// One click is one change and so one step to take back. A drag would be a hundred
    /// commands unless the whole gesture became one, which is not written yet - the arrow
    /// keys do the fine work instead.
    fn click_map(&mut self) {
        // a handle under the pointer is not the map. This comes first, because the whole point
        // of a gizmo is that dragging an arrow moves the object along that arrow - and the
        // tool's own click would put it under the crosshair instead
        if let Some(handle) = self.hot {
            if self.start_drag(handle) {
                self.panels.say(format!("{} {}", host::ui::tr("the gizmo is holding:"), host::ui::tr(&handle.name())));
                return;
            }
        }
        let (picked, aim) = {
            let Some(view) = self.view.as_ref() else { return };
            // `camera().forward()` and not `view.forward()`: the same direction, in the
            // single-precision the world picks with
            (host::pick(view.world(), view.camera().position, view.camera().forward()), self.aim)
        };
        let tool = self.panels.tool;
        match tool {
            Tool::Select => {
                let Some(session) = self.session.as_mut() else { return };
                session.select(picked);
                let line = match picked {
                    Some(_) => session.selection_summary(),
                    None => "Nothing under the crosshair.".to_string(),
                };
                self.panels.say(line);
            }
            Tool::Delete => match picked {
                Some(id) => {
                    if let Some(session) = self.session.as_mut() {
                        session.select(Some(id));
                    }
                    self.act(|s| s.set_selected_deleted(true));
                }
                None => self.panels.say("Nothing under the crosshair."),
            },
            Tool::Move => match (self.selected(), aim) {
                (None, _) => self.panels.say("Choose something first."),
                (_, None) => self.panels.say("The crosshair is not on any ground."),
                (Some(_), Some(at)) => self.act(move |s| s.move_selected_on_the_ground(at)),
            },
            Tool::Turn => match (self.selection(), aim) {
                (None, _) => self.panels.say("Choose something first."),
                (_, None) => self.panels.say("The crosshair is not on any ground."),
                (Some(sel), Some(at)) => {
                    let p = sel.position();
                    // heading 0 is north (+y) and grows clockwise, as the map's headings do
                    let deg = (at.x - p.x).atan2(at.y - p.y).to_degrees();
                    self.act(move |s| s.turn_selected_to(deg));
                }
            },
            Tool::Place => match (self.selected(), aim) {
                (None, _) => self.panels.say("Choose the object to copy first."),
                (_, None) => self.panels.say("The crosshair is not on any ground."),
                (Some(template), Some(at)) => self.act(move |s| s.place_copy(template, None, Some(at), 0.0)),
            },
            Tool::Raise => {
                let step = self.panels.step();
                let down = self.modifiers.shift_key();
                match aim {
                    Some(at) => self.act(move |s| s.raise_ground(at, if down { -step } else { step })),
                    None => self.panels.say("The crosshair is not on any ground."),
                }
            }
            Tool::Flatten => match aim {
                Some(at) => self.act(move |s| s.flatten_ground(at, None)),
                None => self.panels.say("The crosshair is not on any ground."),
            },
        }
        // whatever was read for the object that was chosen before is no longer right
        self.panels.selection_changed();
    }

    /// What is chosen, and what was chosen - read through the session when there is one.
    fn selected(&self) -> Option<i64> {
        self.session.as_ref().and_then(|s| s.selected_id())
    }

    // ---- the gizmo ---------------------------------------------------------------------

    /// The picture, as the gizmo measures it: the camera that draws it, the origin it is drawn
    /// from, and the size of the target in pixels. `None` before a map is open.
    ///
    /// The same camera and the same origin the frame is drawn with, so a handle is where it
    /// looks like it is. A gizmo projected with a slightly different matrix is a few pixels
    /// from where it is drawn - which is a few pixels of a drag in the wrong direction.
    fn screen(&self) -> Option<Screen> {
        let (view, scene, surface) = (self.view.as_ref()?, self.scene.as_ref()?, self.surface.as_ref()?);
        Some(Screen::new(view.camera(), scene.render_origin, surface.config.width, surface.config.height))
    }

    /// The pointer in physical pixels, which is what the frame is drawn in.
    fn pointer(&self) -> Vec2 {
        Vec2::new(self.cursor.0 as f32, self.cursor.1 as f32)
    }

    /// Where the pointer is in the frame's own -1..1 coordinates, which is how a ray is cast.
    fn pointer_ndc(&self) -> (f32, f32) {
        let (w, h) = self
            .surface
            .as_ref()
            .map(|s| (s.config.width.max(1) as f32, s.config.height.max(1) as f32))
            .unwrap_or((1.0, 1.0));
        (self.pointer().x / w * 2.0 - 1.0, 1.0 - self.pointer().y / h * 2.0)
    }

    /// The width over height the frame is drawn at - what a ray needs to be cast.
    fn aspect(&self) -> f32 {
        match self.surface.as_ref() {
            Some(s) => s.config.width.max(1) as f32 / s.config.height.max(1) as f32,
            None => 1.0,
        }
    }

    /// The handle the pointer is on, if any - what a press would take hold of.
    fn pick_handle(&self) -> Option<Handle> {
        let (gizmo, screen) = (self.gizmo_now?, self.screen()?);
        gizmo.pick(&screen, self.kind_now, self.pointer(), self.hot)
    }

    /// Take hold of a handle: from here the object follows the pointer until the button comes
    /// up, and the whole drag is one step to take back (see `Session::begin_gesture`).
    fn start_drag(&mut self, handle: Handle) -> bool {
        let (Some(gizmo), Some(view)) = (self.gizmo_now, self.view.as_ref()) else { return false };
        // a turn is measured from where on the object's own plane the pointer was, so that the
        // object turns by the angle the pointer moved rather than jumping to face it
        let from = if handle == Handle::Turn {
            match view.plane(self.pointer_ndc(), self.aspect(), gizmo.at.z) {
                Some(p) => p,
                // edge-on to the ground: there is no place on the plane that could have been
                // grabbed, and a turn that jumped on its first frame is worse than none
                None => return false,
            }
        } else {
            DVec3::ZERO
        };
        let Some(start) = self.selection().filter(|s| !s.edit.deleted) else { return false };
        let Some(session) = self.session.as_mut() else { return false };
        // the session can refuse - nothing chosen, or a drag already in hand
        if session.begin_gesture().is_none() {
            return false;
        }
        self.drag = Some(Drag { handle, gizmo, grab: self.pointer(), start, from });
        true
    }

    /// One frame of a drag: the object goes where the handle says.
    ///
    /// Nothing is recorded per frame - the drag writes straight into the document and the
    /// history hears about it once, when the button comes up.
    fn drag_step(&mut self) {
        let (Some(drag), Some(view)) = (self.drag.as_ref(), self.view.as_ref()) else { return };
        let edit = match drag.handle {
            Handle::Slide(axis) => {
                let Some(screen) = self.screen() else { return };
                // the axis is a line on the screen; how far the pointer came along it, in
                // metres, is how far the object goes along the map's own axis
                let Some(metres) = drag.gizmo.slide(&screen, axis, drag.grab, self.pointer()) else { return };
                ObjectEdit { moved: drag.start.edit.moved + axis.dir() * metres, ..drag.start.edit }
            }
            Handle::Turn => {
                let Some(now) = view.plane(self.pointer_ndc(), self.aspect(), drag.gizmo.at.z) else { return };
                let degrees = gizmo::turn(drag.gizmo.at, drag.from, now);
                ObjectEdit { turned: drag.start.edit.turned + degrees, ..drag.start.edit }
            }
        };
        let Some(session) = self.session.as_mut() else { return };
        session.drag_to(edit);
    }

    /// Let go: the whole drag becomes one step in the history, said out loud in the console. A
    /// drag that ended where it began is no step at all, and says nothing - which is the
    /// difference between a click that moved nothing and an edit to take back.
    fn end_drag(&mut self) {
        self.drag = None;
        let said = match self.session.as_mut() {
            Some(session) => match session.end_gesture() {
                Ok(line) => line,
                Err(e) => Some(format!("? {e}")),
            },
            None => None,
        };
        if let Some(line) = said {
            self.panels.say(line);
        }
        // the pointer is over the map again, and has to say which handle it is on afresh
        self.hot = None;
        self.gizmo_now = None;
    }

    fn selection(&self) -> Option<omsi_editor_core::Selection> {
        self.session.as_ref().and_then(|s| s.selection())
    }

    /// A command from the keyboard, run through the console's own path so that what it did
    /// is said in the console like anything else.
    fn console(&mut self, line: &str) {
        let Some(session) = self.session.as_mut() else { return };
        self.panels.run(session, line);
    }

    /// Do something to the session, with what it says going to the console.
    ///
    /// Every panel action goes through here, so none of them can fail quietly - and so the
    /// screen is brought up to date in one place rather than in each of them.
    fn act(&mut self, f: impl FnOnce(&mut Session) -> Result<Option<String>, omsi_editor_core::EditError>) {
        let Some(session) = self.session.as_mut() else { return };
        self.panels.act(session, f);
    }

    /// The Maps page asked for a map that is not there yet.
    ///
    /// Making one is the one thing the page can ask for that the core cannot do: a new map is
    /// a folder of its own with a `global.cfg`, a tile list and the terrain under it, and none
    /// of that is written. It says so rather than opening nothing.
    fn make_a_map(&mut self) {
        if self.panels.take_new_map() {
            self.panels.say(host::ui::tr("Making a new map is not written yet.").to_string());
        }
    }

    /// The first page asked for a map: open it, and the page is over.
    fn open_the_chosen_map(&mut self) {
        let Some(cfg) = self.panels.open_map.take() else { return };
        self.open_map(&cfg);
    }

    /// The write chip in the top bar asks for the other destination: open the session again
    /// there, on the same map. The panel sets this only when nothing is unsaved, so a change
    /// can never be dropped by it - and the history, which belongs to the session, goes with
    /// the session it was made in.
    fn reopen(&mut self) {
        let Some(destination) = self.panels.reopen.take() else { return };
        let Some(map_cfg) = self.map_cfg.clone() else { return };
        match Session::open(&map_cfg, destination.clone()) {
            Ok(s) => {
                self.session = Some(s);
                self.shown = Shown::default();
                self.panels.say(match &destination {
                    Destination::Content(p) => format!("{}\n{}", host::ui::tr("A save writes copies under"), p.display()),
                    Destination::InPlace => host::ui::tr("A save writes the map's own files, with a .openomsi-bak snapshot each").to_string(),
                });
            }
            Err(e) => self.panels.say(format!("{}: {e}", host::ui::tr("could not open the map again"))),
        }
    }
}

/// A direction with its height taken out: an arrow key moves along the map.
fn flat(v: DVec3) -> DVec3 {
    let v = DVec3::new(v.x, v.y, 0.0);
    if v.length_squared() > 1e-9 {
        v.normalize()
    } else {
        DVec3::Y
    }
}

/// The tool a key chooses, if it is one of them.
fn tool_key(code: KeyCode) -> Option<Tool> {
    match code {
        KeyCode::KeyV => Some(Tool::Select),
        KeyCode::KeyM => Some(Tool::Move),
        KeyCode::KeyR => Some(Tool::Turn),
        KeyCode::KeyC => Some(Tool::Place),
        KeyCode::KeyX => Some(Tool::Delete),
        KeyCode::KeyG => Some(Tool::Raise),
        KeyCode::KeyH => Some(Tool::Flatten),
        _ => None,
    }
}

/// The key as the widgets know it, for the ones they care about: a text field's caret, a
/// dropdown's arrows. The rest the window handles itself.
fn widget_key(code: KeyCode, ctrl: bool) -> Option<PanelKey> {
    match code {
        KeyCode::ArrowLeft => Some(PanelKey::Left),
        KeyCode::ArrowRight => Some(PanelKey::Right),
        KeyCode::ArrowUp => Some(PanelKey::Up),
        KeyCode::ArrowDown => Some(PanelKey::Down),
        KeyCode::Home => Some(PanelKey::Home),
        KeyCode::End => Some(PanelKey::End),
        KeyCode::Backspace => Some(PanelKey::Backspace),
        KeyCode::Delete => Some(PanelKey::Delete),
        KeyCode::Enter | KeyCode::NumpadEnter => Some(PanelKey::Enter),
        KeyCode::Escape => Some(PanelKey::Escape),
        KeyCode::Tab => Some(PanelKey::Tab),
        KeyCode::KeyA if ctrl => Some(PanelKey::SelectAll),
        KeyCode::KeyC if ctrl => Some(PanelKey::Copy),
        KeyCode::KeyV if ctrl => Some(PanelKey::Paste),
        KeyCode::KeyX if ctrl => Some(PanelKey::Cut),
        _ => None,
    }
}

impl ApplicationHandler for Editor {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.window.is_none() {
            self.open_window(event_loop);
        }
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
        match event {
            WindowEvent::CloseRequested => event_loop.exit(),

            WindowEvent::Resized(size) => {
                if let (Some(surface), Some(renderer)) = (self.surface.as_mut(), self.renderer.as_ref()) {
                    surface.resize(renderer, size.width.max(1), size.height.max(1));
                }
            }

            WindowEvent::ModifiersChanged(m) => {
                self.modifiers = m.state();
                let input = self.panels.input();
                input.shift = self.modifiers.shift_key();
                input.ctrl = self.modifiers.control_key();
                input.alt = self.modifiers.alt_key();
            }

            WindowEvent::KeyboardInput { event, .. } => {
                let PhysicalKey::Code(code) = event.physical_key else { return };
                let (shift, ctrl) = (self.modifiers.shift_key(), self.modifiers.control_key());
                match event.state {
                    ElementState::Pressed => {
                        let fresh = self.held.insert(code);
                        // the panels get the keys as well as the window does: a letter typed
                        // at the console is a letter, and `Enter` there is a command
                        {
                            let input = self.panels.input();
                            if !ctrl {
                                if let Some(t) = event.text.as_deref() {
                                    input.text.push_str(t);
                                }
                            }
                            if let Some(k) = widget_key(code, ctrl) {
                                input.keys.push(k);
                            }
                            if fresh {
                                input.raw_key = Some(code);
                            }
                        }
                        if !fresh {
                            return;
                        }
                        // the start page: the keys are a list's, not a map's
                        if self.session.is_none() {
                            match code {
                                KeyCode::ArrowDown => self.panels.choose(1),
                                KeyCode::ArrowUp => self.panels.choose(-1),
                                KeyCode::Enter | KeyCode::NumpadEnter => self.panels.open_chosen(),
                                KeyCode::Escape => event_loop.exit(),
                                _ => {}
                            }
                            return;
                        }
                        // typing at the console is not flying, and `M` there is a letter
                        let typing = self.panels.typing();
                        if ctrl {
                            // the commands, on the keys everyone reaches for
                            match code {
                                KeyCode::KeyS => self.console("save"),
                                KeyCode::KeyZ if shift => self.console("redo"),
                                KeyCode::KeyZ => self.console("undo"),
                                KeyCode::KeyY => self.console("redo"),
                                _ => {}
                            }
                            return;
                        }
                        if typing {
                            return;
                        }
                        if let Some(tool) = tool_key(code) {
                            self.panels.tool = tool;
                            return;
                        }
                        match code {
                            KeyCode::Escape => event_loop.exit(),
                            // F: the whole map at once, rather than the tiles around the
                            // camera - how a modder looks at a map's layout as a whole
                            KeyCode::KeyF => {
                                if let (Some(view), Some(renderer), Some(scene)) =
                                    (self.view.as_mut(), self.renderer.as_ref(), self.scene.as_mut())
                                {
                                    view.stream_all(renderer, scene);
                                    let note = view.note.clone();
                                    self.panels.say(note);
                                }
                            }
                            KeyCode::ArrowLeft
                            | KeyCode::ArrowRight
                            | KeyCode::ArrowUp
                            | KeyCode::ArrowDown
                            | KeyCode::PageUp
                            | KeyCode::PageDown => self.nudge(code),
                            _ => {}
                        }
                    }
                    ElementState::Released => {
                        self.held.remove(&code);
                    }
                }
            }

            WindowEvent::MouseInput { state, button: MouseButton::Left, .. } => {
                let pressed = state == ElementState::Pressed;
                self.left_down = pressed;
                let input = self.panels.input();
                input.down = pressed;
                input.pressed = pressed;
                input.released = !pressed;
            }

            WindowEvent::MouseInput { state, button: MouseButton::Right, .. } => {
                let pressed = state == ElementState::Pressed;
                {
                    let input = self.panels.input();
                    input.right_down = pressed;
                    input.right_pressed = pressed;
                }
                // a right click on a panel is not a look: the panels come first
                self.looking = pressed && self.session.is_some() && !self.panels.over_ui();
            }

            WindowEvent::CursorMoved { position, .. } => {
                let scale = self.window.as_ref().map(|w| w.scale_factor()).unwrap_or(1.0);
                let now = (position.x / scale, position.y / scale);
                let (dx, dy) = (position.x - self.cursor.0, position.y - self.cursor.1);
                self.panels.input().mouse = glam::Vec2::new(now.0 as f32, now.1 as f32);
                if self.looking {
                    if let Some(view) = self.view.as_mut() {
                        // the pointer moves in physical pixels; the view turns the way it does
                        // in the game, at the game's own gain (see `view::turn`)
                        view.look(dx, dy);
                    }
                }
                self.cursor = (position.x, position.y);
            }

            WindowEvent::MouseWheel { delta, .. } => {
                let ticks = match delta {
                    MouseScrollDelta::LineDelta(_, y) => y as f64,
                    MouseScrollDelta::PixelDelta(p) => p.y / 50.0,
                };
                // over a panel the wheel scrolls it; over the map, with a ground tool in
                // hand, it grows the brush; otherwise it is the camera's speed
                if self.panels.over_ui() || self.session.is_none() {
                    self.panels.input().wheel.y += ticks as f32;
                } else if self.panels.tool.is_brush() {
                    let factor = 1.25f64.powf(ticks);
                    if let Some(session) = self.session.as_mut() {
                        self.panels.brush_by(session, factor);
                    }
                } else if let Some(view) = self.view.as_mut() {
                    view.change_speed(ticks);
                }
            }

            WindowEvent::RedrawRequested => {
                let now = Instant::now();
                let dt = (now - self.last).as_secs_f64().min(0.1);
                self.last = now;
                if dt > 0.0 {
                    self.fps = self.fps * 0.9 + (1.0 / dt as f32) * 0.1;
                }
                self.tick(dt);
                // where the crosshair is, before the frame that shows it is drawn
                self.aim = self.view.as_ref().and_then(|v| v.aim());
                // the frame draws the gizmo where it stands this one, and reads which handle
                // the pointer is on where it was drawn last
                self.draw(dt as f32);
                if self.drag.is_some() {
                    // a drag runs while the button is down, one step a frame. The pointer may
                    // pass over a panel on the way and the object keeps following it: a drag
                    // that stops dead at the edge of a panel is a drag that has been lost
                    if self.left_down {
                        self.drag_step();
                    } else {
                        self.end_drag();
                    }
                } else if self.session.is_some() && !self.panels.over_ui() {
                    // which handle the pointer is on - nothing over the panels, where the
                    // gizmo is behind them
                    self.hot = self.pick_handle();
                }
                // the panels have been laid out by now, so "was the map clicked" is known -
                // and the frame has used the raw input up, which is why it is answered inside
                if self.panels.clicked_map {
                    self.click_map();
                }
                self.open_the_chosen_map();
                self.make_a_map();
                self.reopen();
                if let Some(w) = self.window.as_ref() {
                    w.request_redraw();
                }
            }

            _ => {}
        }
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        // `quit` at the console closes the window with it, which is what it has always meant
        if self.panels.quit {
            self.closing = true;
        }
        if self.closing {
            event_loop.exit();
        }
    }
}
