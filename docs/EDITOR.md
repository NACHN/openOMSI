# The map editor

openOMSI is three programs: the game (`openomsi`), the launcher (`openomsi-launcher`) and the
editor (`openomsi-editor`). This page is about the third one - what it can do today, and the
order the rest of it is going to arrive in.

It is the counterpart of the "Still open" lines in [ARCHITECTURE.md](ARCHITECTURE.md). Those
record what each round *did*; this records what is *planned*, and why in that order. When a
stage here lands, ARCHITECTURE.md gains the round that did it and this page loses the stage.

## The shape of it

The editor is a window (`crates/omsi-editor`) over a headless kernel
(`crates/omsi-editor-core`). The kernel can open a map, edit it and write it back with no
window, no GPU and no simulation in it, and it is tested from a terminal; the window decides
*when* a thing is asked for, never *what* it is. The window and the terminal prompt drive the
same `Session`, so a click cannot do something a line cannot.

Two rules the code already keeps, and that every stage below keeps:

- **Nothing invents a control the core cannot do.** A panel that offers what `Session` cannot
  perform is a lie told in pixels. It is why there is no spline tool today, and why the asset
  browser waited for stage 2 rather than being drawn empty.
- **A tile file is edited as text.** An edit rewrites only the lines it must and passes every
  other byte through untouched - comments, unknown keywords, line endings and the encoding
  included. A parse/write round trip would drop what mods add. `record.rs` is the whole of
  that rule for `[object]` records, and its tests assert it byte for byte.

## Where it stands

What works now, in one paragraph: a map opens and is drawn as the game draws it; eight tools
(choose, move, turn, place, take away, raise the ground, flatten it, and the tile grid) act on one
`[object]` at a time - or, for the last of them, on the map's own tiles; the chosen object is
dragged by a gizmo with move arrows and a turn ring, and a whole drag is one step to take back; a
save writes copies under the content folder or the map's own files, and never the installation; a
map can be made from nothing, a tile added to it or taken out of it by clicking the map itself, and
an object placed in it out of the content folder even when no map holds that file yet, with
`global.cfg` and every tile edited line by line like every other file; and the panels are the
launcher's own widgets - a rail of tools, a dock of five pages (inspector, outline with its objects
and its tiles, assets, history, unsaved), a tool options flyout, a status bar and a console.

What is missing is easier to list, and each line is a stage below. There is no road or track at
all. There is one selection. There is no snapping. The ground can only be raised and flattened.
The camera is a fly-through, with no plan view. An asset is shown in 3D once it is armed, and the
list itself is still names and folders - a thumbnail per row would mean reading every `.sco` in
the installation. And every tile of a map can be ringed on the map, but there is no picture of
one: what a tile *holds* - its objects, its ground - is read from the list and the console, not
drawn on the ground it belongs to.

## The order

### 0. One place for each thing

**Why before everything else.** The cost of a shape is the number of things already written in
it, and today that number is seven tools and two kinds of editable record. Stage 2 adds a third
record shape and stages 3 and 4 add a whole new *kind* of editable thing, so the shaping is
cheapest now.

**Why not "because it is getting long".** It is not, and length is not an argument in this
project: `omsi-editor/src/ui.rs` is 1479 lines, `omsi-app/src/scene.rs` is 13512 and
`omsi-render/src/lib.rs` is 12447. The three arguments below are about things being written
twice, not about size.

**(a) The editing model exists twice - and the second copy is frozen.** `omsi-editor-core`'s
`Document`, `Session`, `Command` and `History` are driven by the editor window and by the
terminal prompt, and by nothing else. The game's own object editor (`omsi-app/src/editor.rs`,
455 lines) keeps its own list of what was changed, its own `Added` record where the core has
`NewObject`, and its own save loop; it shares only the byte-level writing end (`rewrite_tile`,
`add_copies`, `ground::shape`) and `ObjectEdit`. So the middle of an edit - what a copy is,
what "changed" means, how a save is written - is written twice.

**The decision is not to merge them, because the in-game editor is on its way out.** A
program of its own, with a rail, a dock, an undo stack and a plan view, is the editor this
project is building; the in-game one was the first attempt at it, and once the standalone one
can do everything the in-game one can, the in-game one has no reason to exist. So:

- **The in-game editor is frozen.** It keeps working and it keeps being fixed when it breaks -
  it is what `Ctrl+Shift+E` does today, and USER_GUIDE.md documents it - but no feature is
  added to it again. Every stage below is the standalone editor's, and none of them is
  mirrored there. In particular stage 2 gives the standalone editor the ability to place a
  `.sco` that no map holds, and deliberately does **not** give it to the in-game one; that is
  a capability gap between them, and the reason the standalone one exists.
- **Nothing in the core depends on the in-game editor**, so the removal is a deletion of
  `omsi-app/src/editor.rs` and its keys, with the byte-level writers and `ObjectEdit` staying
  where they are because the standalone editor uses them too. That is the test of whether the
  split was drawn in the right place, and it was.
- **The one thing the in-game editor can do and the standalone cannot** is edit against a
  world that is being simulated - a bus at a stop, traffic, people - because the standalone one
  opens the map without them (`host::open`: "minus the traffic, the timetable and the player").
  If that turns out to be wanted after all, it is an argument for a *play-and-edit* mode in the
  standalone editor, not for keeping a second editor alive.
- Until the removal happens, the two writers share only the byte level, so a change there has
  to be considered for both. That is the whole of the maintenance cost this decision accepts.

What this means for the stages below: (a) is a decision rather than work, and it is written
here so that nobody re-opens it by migrating the in-game editor onto `Session`, which is the
obvious-looking move and the wrong one.

**(b) A tool is spelled out in four places.** Adding one tool today means: a variant plus eight
methods in `ui.rs` (`icon`, `name`, `key`, `mark`, `colour`, `hint`, `is_brush`, `ALL`); its
click behaviour in `window.rs::click_map`, about fifty lines that no test reaches; and its
handles in `gizmo.rs::kind_for`. Two of the four are already settled - the key that chooses a
tool and the word `--ui-tool` takes are both read back out of `Tool::key` and `Tool::word`
rather than kept in a second table, so the tip the rail prints and the key that works cannot
come apart any more, and a word that is not a tool is refused rather than quietly answered
with `select`.

The shape to aim at is `src/tools/` with one file per tool, and the list of tools as the only
place a tool is enumerated, so that `Tool::ALL` derives the rail, the key map, the usage
message and the `--ui-tool` names. Adding the spline tool is then one new file. (What an
outline rings a chosen object with is already right: it goes through `Tool::mark`.)

**(c) Split `ui/` by panel, the way the launcher already is.** `crates/omsi-app/src/launcher/`
is the precedent: one file per page (`drive`, `mobile`, `multiplayer`, `phone`, `showroom`,
`timetable`, `update`) beside what the pages share (`mod`, `ui`, `theme`, `state`). `ui.rs` is
now four jobs in one file - the tool enum and how it is presented, the layout and the GPU
upload plumbing, six panels, and the console - and stage 2 makes it seven panels. So `src/ui/`
with a file per panel, `dock/` with a file per dock page, and a `mod.rs` holding only the
layout and the plumbing.

**(d) What is not planned.** Not new crates: the boundary that matters - headless kernel
versus window - is already drawn, and `omsi-editor-ui` or `omsi-editor-tools` would buy
nothing while making the `openomsi_game::host::ui` re-export story worse. And not a rewrite:
(a) is a migration, (b) and (c) move code without changing what it does.

**Done when:** a tool's key, its icon, its tip, its gizmo and what its click does are all
reachable from one new file plus one line in a list; the in-game editor is marked frozen where
someone would look for it, so that no feature is added to it by mistake; and `ui/` holds a file
per panel with the same behaviour as before - which `--ui-shot` can show, because it draws
every page.

### 1. Several things at once

**Why next.** Every gesture today goes through `Session` and ends in one `Command`. Turning
`Selection` from one id into a set, and a gesture into a `Command::Batch`, is cheap while there
are seven tools and expensive once there are eleven - the same argument as stage 0, one level
down.

- the core: `Selection` becomes a set of ids; `move_selected`, `turn_selected`,
  `set_selected_deleted` and `reset_selected` act on all of them through `Command::Batch` -
  whose inverse is already worked out by reversing the inverses of its parts.
- the view: a box select, and Shift+click to add to the choice.
- snapping, here because it is the same kind of arithmetic and touches the same call: a grid
  step for a move, an angle step for a turn, and "snap to the nearest object" within a radius.
- the panel: the inspector says how many are chosen and what can be done to the group, and a
  position and a heading can be typed exactly (`move_selected_to` / `turn_selected_to` already
  take one).

**Done when:** moving three objects in one drag is one step to take back, a box select chooses
what the outline lists, and a typed `195.84` puts an object exactly there.

### 2. Growing a map

**Why second.** It is the line between arranging a map and making one, and it has two halves:
the tiles the map is *made of*, and the objects that *stand on* them. Both are about a file
that does not exist yet, which is what makes them one stage.

**(a) The map's own tiles - done.** A tile is a `[map]` entry in `global.cfg`, a
`tile_x_y.map` and a `tile_x_y.map.terrain`. Adding one and taking one away are not the same
size of edit, and the difference is the whole of this half:

- **Adding is an append**, so nothing that names a tile changes its number. Safe by
  construction.
- **Taking one away is not**, because a tile is named by its *place* in the `[map]` list - by
  an entry point's `group` and by a timetable track's `[track_entry]` (see
  `GlobalMap::raw_tiles` in `omsi-map`, and `scene.rs`/`schedule.rs`, which are the two places
  that look a tile up by number). Removing an entry from the middle renumbers every entry after
  it, and anything still naming a later number would go on naming it and mean a different tile.
  So the removal rewrites the entry points, which are in the same file, and **refuses** a
  removal that renumbers anything when the map has any track file at all - this project does not
  write `.ttr`, so a route pointing at the wrong tile is a bug no one would find by looking at
  the map. An entry point standing *in* the tile that goes is refused too: it is where the
  player starts, and there is nowhere sensible to move it to.
- **A tile's file is never deleted.** It is unlisted, and putting the tile back restores it
  exactly - file text, the objects it listed, what this session had done to each, and its
  ground. Deleting somebody's file is not an edit to undo.

Written as `tilemap.rs` - the same rule `record.rs` keeps, one level up: the lines it must
change, and every other byte of `global.cfg` passed through.

**And changed on the map itself.** Which tiles a map has is the one thing about it that was
invisible: the ground is drawn as far as the tiles go, and one more tile of it would look exactly
like the ground already there. So the last piece of this half is the **tile tool** (T): it rings the
tiles where they are - the map's own in red, the ring of tiles it could grow into in white, the one
under the pointer in green - and a click adds the tile it is on or takes out one the map has. The
colours are `omsi_render::Mark`'s own, the ones a click acts in, so the ring round a tile and the
tool button that put it there cannot disagree; and the tile under the pointer *when the map already
has it* is marked out by a frame drawn inside it rather than by a fourth colour, because there are
four and each has to keep meaning one thing - green is a click that adds, and a tile the map has is
never one of those. Almost nothing had to be added to the renderer for it: the rings are the outline
pass the chosen object is already ringed with, which is what makes a tile's border the same three
pixels wide from any height, and it draws with no depth, so a tile behind a building still has its
border instead of a hole in the grid. What did have to be added is one flag,
`Instance::outline_only`: the mask pass collapses an instance that is not `visible` (`vs_outline`),
so a mark that is to be ringed and never drawn had to be able to say exactly that. "Not drawn" could
not be spelled with the flags that were there - an invisible instance is not outlined either, and
one that is drawn to be outlined is a pane the size of a tile laid over the ground. The console
(`tiles`, `tile add x y`, `tile rm x y`) and the outline's **Tiles** half are still there for what a
click cannot say - a tile by its number, and why a removal was refused.

**And it shows up without the map being opened again.** A tile added to the list used to need a
reload before the ground followed, and a tile taken out went on being drawn: `World` holds its own
parse of `global.cfg`, taken when the map was opened, and streams the tiles *that* names. So the
session's own parse is handed over (`Document::global_now`, `View::adopt_tiles`) and the streaming
that is already there does the rest in the same frame - a tile that went is outside the area wanted
and comes off the screen, one that came is inside it and is built. That includes the entry points:
taking a tile out of the middle of the list renumbers the ones after it, so a world told only the
shorter list would have an entry point standing in a tile that is no longer in the map. A tile added
and *not yet written* is the one case that stays invisible, and deliberately: it is in the map's list
and has no file, and a file is what a tile is built from - which is what the tiles page means by
"not written yet", and why the ground appears on the save. That used to be the whole of it: the page
said "not written yet" on the row and nothing anywhere said what to do about it, so the way to see a
tile you had just added was to work out that `save` was the answer. Now what is waiting and the
button that writes it are together - on the tiles page, and in the **tile tool's own options**, which
is beside the button that chose the tool and where the eye already is after clicking the map. Writing
is the map's own save, so it takes whatever else is waiting with it, and the note under the count
says so. The editor's own save destination is registered as a content root (last, so it can shadow
nothing) for the same reason: the copy a save writes under the content folder is the file the game
reads, so it has to be the file the editor reads back.

There was a second cache in the way, and it is the one worth remembering: `World`'s **tile layout**
(`scene.rs`) is built once, on first use, out of the tiles that are *there* - each tile's file, its
neighbours, the tiles whose splines reach it - and every load goes through it. A tile with no place
in that layout is staged as nothing and drawn as nothing, **without a word**: the count of tiles built
went up, the tile was in every list, and the picture did not change. So the layout is let go too
(`World::forget_layout`, which drops the staged tiles with it) whenever the tiles that are there have
changed - and that is asked of the files rather than remembered, because what makes a tile a tile is
that its file is there. It is also what makes a *saved* tile appear: until the save it was a tile that
could not be built at all, so the layout made before it had no place for it either.

**(b) Placing what is not there yet - done.** This is what "a map can grow" means on the
object side: everything else in the editor arranges what is already there. An object out of
the content folder is the one thing with nothing to copy, and the record is written from
nothing.

- **The record.** `record::blank_object_record` writes it: the keyword, the detail level, the
  file, the id, the tile-local `x`, `y`, `z`, the heading, and the two angles and the label
  count a version 12 and later tile carries - eleven lines, with the label list empty. (Note
  eleven and not thirteen: the field list is what `omsi-map`'s reader reads, and a first pass
  at this put two more lines in - a blank and a name - which read back as nothing at all. The
  truth is `tile.rs`'s `read_labels`, not what a tile file looks like.) The place is
  *tile-local* and `z` is `0`, because a record's coordinates are against its tile and its `z`
  is a height **over** the ground - write the ground's height there and the object floats.
- **The list.** `assets::scenery_objects` walks `Sceneryobjects` and returns what a record
  would have written (`Sceneryobjects\Berlin\House1.sco`); through `omsi_cfg::mirrored_dirs` and
  the vfs listing, so an object inside an archive in `Archives` is listed beside the ones lying
  on disk, and to a depth limit, so a folder joined to itself cannot hang the walk.
- **The session.** `Session::arm_asset` puts a `.sco` in the place tool (tool state like the
  brush, so it is not a step to take back), and `Session::place_asset` resolves the file,
  refuses one the content folder does not have and a tile the map does not list, and places the
  object as one step to take back. The console reaches all of it: `assets [text]`, `arm <sco>`
  and `new <sco> <x> <y> [h]`.
- **The panel.** A fifth dock page, **Assets**: the list, a search box, a refresh, and a click
  that arms the tool and puts it in hand. The armed file is shown where it can be seen from
  the list it came out of and in the tool options, and clicking it again - or Escape, or the
  close on either chip - lets it go. The one list in the panels that is not read from the map,
  so it is read once and kept: it is a whole folder tree walked.
- **The picture.** A row is a name and a folder, and what a scenery object *looks* like is the
  first thing anyone wants to know about it - so the page shows it, in 3D, by reusing the
  launcher's **showroom** rather than growing a second preview: `Look` gained an `object`, and
  with a bus empty and an object set the same floor, sky, light, orbit and zoom stand that `.sco`
  on the floor instead of a vehicle. It is reachable through `host::showroom`, the editor's one
  door into `omsi-app`. The camera frames whatever it is given by its box's half-diagonal and
  half-height, because one number for both cuts something off - a bus is long and flat, a water
  tower is tall and narrow - and the pointer over the picture turns it and the wheel comes
  closer, as on the launcher's own card. Reading every `.sco` to draw a thumbnail per row would
  be a windowful of them at once; reading the one that was picked, once, is not.
- **The tool.** `Place` has two sources and says which: a copy of the selection, or the armed
  file. The status line changes with it, because a click on the ground that puts a different
  thing down than last time must not have to be guessed at.

**Done when:** the map's own list of tiles can be changed without anything that numbers a tile
being left wrong, and a `.sco` that no map holds can be placed in a tile added for it, saved,
and found in the tile file after a reopen with every other byte of that file unchanged - which
is exactly what the tests assert, and the "after a reopen" half is what caught the two places
where a tile was read from somewhere other than where a save would write it:

- `Document::index` resolved a tile's file with `resolve_path(map_dir, name)`, which finds the
  content folder's copy only when the launcher has a content root registered. A tile an earlier
  session *added* is in no installation at all, so a map reopened without the launcher never saw
  what was in it - silence, not an error. It now goes through `Document::tile_source`, which is
  the file a save would write that tile in.
- `content_copy` measured a file's place under the content folder from the installation, so
  handed a path that was *already* the copy - which is exactly what `tile_source` returns - it
  put the next save beside the map folder instead of inside it. A file already under the
  destination is now handed back as it is.

### 3. Roads and tracks

**Why third.** It is the largest gap, ARCHITECTURE.md already names it as open, and the
reading half needs nothing from the engine: `omsi-map` parses a `[spline]` in full - `file`,
`id`, `prev_id`, `next_id`, position, heading, length, radius, both gradients, the height
change, cant and skew - and `omsi-scenery::sli` already reads the cross-section a spline is
drawn with. What is missing is a *writing* side.

That writing side has three traps worth writing down before anyone starts.

- **The record's fields are not all at a fixed offset from the keyword.** The file line is
  `[spline]` followed by the detail level for version 9 and later, but is the first line for
  older versions; `prev_id` and `next_id` exist only from version 11, and before that a single
  line links a spline to the one written before it. So a rewriter must find the file line by
  counting from the tile's version, then count the fields it edits from *there*.
- **A spline stores x, height, y - in that order.** Not x, y, z. The engine's own comment says
  why (it is what the chains of stock maps prove). Every other coordinate in this project is
  x, y, z, so this is exactly where an object's and a spline's axes will be silently swapped.
- **The tail is variable.** `[spline_h]` carries one more number than `[spline]`; cant arrives
  at version 5 and skew at 14; the count of trailing numbers decides whether the last one is a
  texture offset or a pair of skews and an offset; and `mirror` may follow any of it. The tail
  must be passed through untouched, never re-emitted.

- the core: `spline.rs`, a text-level reader and writer for `[spline]` and `[spline_h]`, built
  on the rule `record.rs` keeps and tested the same way.
- the core: `Document` indexes splines as it does objects, so a spline is addressable by id and
  the same commands move it.
- the view: a chosen spline is drawn along its own curve - the length and radius are in the
  record, so the curve is known without the `.sli` - and its chain is drawn dimly, because a
  spline moved on its own leaves a gap.
- the panel: the inspector shows a spline's length, radius, gradients and the two neighbours in
  its chain.

**Done when:** moving a road on a stock map and saving changes that record's lines and no
others, and the chain still lines up afterwards.

### 4. Making new road

**Why fourth.** It depends on stage 3's writer, and it is the first stage that needs engine
work rather than editor work. ARCHITECTURE.md already names the hard part: making or moving a
road piece "regenerates their meshes, lanes and terrain cuts". A tile file that has gained a
`[spline]` is not a map that has a road in it until the geometry, the lanes the AI drives on
and the cut in the terrain have all followed.

- the core: place a new `[spline]` from a `.sli`, and a join that sets `prev_id` and `next_id`
  on both neighbours - the pair must be written together or neither.
- the engine: regenerate the mesh, the lanes and the terrain cut around a changed spline.
- the panel: a **Spline** tool - click to drop points, drag sideways to set the radius, and one
  action to join what the pointer is on to what was just made.

**Done when:** a road drawn on an empty map can be driven over in the game.

### 5. The ground

**Why fifth.** It is small and self-contained, and the shape of it is already right: a brush
stroke is a `GroundAction`, its inverse is the samples it touched, and undoing one puts them
back. Two more actions fit that mould, and one does not.

- `GroundAction::Smooth` and `GroundAction::Slope`, both pure arithmetic in `ground.rs`, both
  undone by the `RestoreTerrain` inverse that already exists.
- a brush shape (a circle today) and a falloff, so a raised hill has sides.
- water, which the map file declares as a plane rather than a height, so it needs its own
  reading and writing before it needs a tool.
- **not** here: painting the ground's texture. The editor's `Terrain` is heights only; the
  texture is a separate `tile_x_y.map.<n>.dds` that nothing in this project reads or writes
  yet. That is its own stage, and a bigger one than it sounds.

**Done when:** a slope from a reference point and a smoothed hillside are each one stroke and
one undo.

### 6. Seeing the map

**Why sixth.** It is the only item that needs the renderer, and everything before it works
without one. Doing it first would have been pleasant and slow; doing it here makes all of the
above quick to use.

`Camera` has a field of view and a near and far plane, and `Renderer` builds its projection
with `Mat4::perspective_rh` on every path. A true plan view is therefore a renderer change - an
orthographic projection for the map pass - and not a camera pose. (A camera looking straight
down from high up is free today and is not the same thing: at 80 m a lamp post still leans away
from the middle of the frame.)

- the renderer: an orthographic option on the projection, for the map pass only.
- the view: a plan mode - straight down, panned by dragging, zoomed by the wheel. The wheel is
  not a zoom today; it changes the camera's flying speed.
- the panel: the top bar's plan / three-dimensional switch, and the minimap, which is the same
  projection in a corner.

**Done when:** a whole tile is on the screen at once, and a click places an object by the same
rule anywhere in the frame.

### 7. Layers, and the rest

The items that are worth having but block nothing else.

- visibility and lock per kind - objects, splines, attachments, ground. The first is a filter
  in the render path and the second a filter in the pick path, and **both must agree**, or a
  hidden object is still clickable.
- the ground's texture: read and write a tile's map layers, then a paint tool.
- timetables and tracks: a line's route (`.ttr`), its stops and their times. The launcher's
  Timetable page already edits lines, tours and departures; what a map editor adds is the stops
  on the ground and the tracks between them.
- the small ones: a measure tool, a lighting and time-of-day preview, a warning when a tile
  changes on disk under an open session, and a shortcut sheet.

## What a stage is not finished without

- `cargo test -p omsi-editor -p omsi-editor-core` passes, and a new kind of record - a spline,
  a fresh object - has the same byte-for-byte pass-through tests `record.rs` has.
- `--ui-shot` draws the new panel. A panel that cannot be looked at without a window cannot be
  reviewed, and `--ui-dock`, `--ui-tool` and `--ui-aim` already exist for this.
- The stage is written down: ARCHITECTURE.md gains the round that did it, and this page loses
  it.

## Open questions

Two decisions belong to whoever owns the map rather than to the code, and both are already
visible in the current behaviour.

- **A save drops the undo history.** It has to: saving moves the baseline the edits were
  measured against, so a step from before it would apply a stale change rather than undo a
  real one. The alternative is a history that survives a save by re-measuring every step
  against the new baseline, which is possible and is a real change to `History`. Until then,
  the history page says so out loud rather than looking broken.
- **Where a save may write.** Today: copies under the content folder by default, the map's own
  files when asked (`--in-place`), and never inside the installation - a save that would land
  there is refused. Whether a map editor should ever be able to write into a shared
  installation is a question about other people's computers.

## A note on the interface

The panels are drawn with the launcher's own widgets (`openomsi_game::host::ui`), in the
launcher's colours, so the three programs show one face. A new panel is a page of the dock, a
new mode is a switch in the top bar, and a new per-tool setting is a row of the tool options
flyout - adding a fourth way of showing something is how this interface would come apart.
