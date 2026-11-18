# Architecture

Hobby Jogger is a Tauri app. Rust owns the machine connection, all application state and every
decision about what may be sent to the machine. The React frontend renders that state and turns
user input into requests; it holds no state of its own beyond view concerns (open panels, input
fields) and the G-code editor's buffer (see [Programs](#programs)).

```
┌──────────────── React (presentation) ────────────────┐
│  store: AppState (immer, patched)   telemetry store  │
│        ▲ state-sync            ▲ telemetry  ▲ console│
└────────┼───────────────────────┼────────────┼────────┘
   invoke│ requests              │ events     │
┌────────┼───────────────────────┼────────────┼────────┐
│ app/src-tauri: thin adapter (commands ⇄ hj-core)     │
├──────────────────────────────────────────────────────┤
│ hj-core: Core actor ── owns AppState, gating, sync   │
│   └─ machine session: polling, command semantics     │
├──────────────────────────────────────────────────────┤
│ hj-makera: frames, status parsing, TCP/USB, upload   │
└──────────────────────────────────────────────────────┘
        hj-sim: simulated Z1 speaking the same protocol
```

## Crates

- **`hj-makera`** — protocol only: frame codec, status/diagnose parsing, transports (TCP, USB
  bulk), discovery, file upload. No application logic.
- **`hj-core`** — the application core. A single actor task owns `AppState`, receives `Request`s,
  talks to the machine through `hj-makera`, and publishes changes through a `Sink` trait so it
  doesn't depend on Tauri.
- **`hj-gcode`** — G-code analysis for the preview and pre-flight checks: interprets a program
  roughly as the Z1 would (modal state, arcs, canned drilling cycles, `G92`, units) into a toolpath
  in work coordinates with source lines, bounds, a time estimate and diagnostics.
- **`hj-sim`** — a simulated Z1 (TCP, framed protocol) implementing the behaviour we measured on
  the real machine: status, `$J`/`$H`, `G53` moves, halt/`$X`, soft limits, `G10 L20`, version/model,
  an in-memory SD card (`ls -s`, `rm`, uploads) and job playback (`play`/`suspend`/`resume`/`abort`).
  Used by tests and by the app's "Simulator" connection.
- **`hj-cli`** — the `hj` protocol exploration tool.
- **`app/src-tauri`** — the Tauri binary: registers commands that forward to `hj-core` and
  implements `Sink` by emitting events.

## State sync

Three channels, chosen by how the data changes:

| Channel | Content | Mechanism |
|---|---|---|
| `state-sync` | `AppState`: connection, machine info, slow-changing status (state, halt, tool, spindle target, offsets, job progress), discovered machines, the SD card listing, upload progress (in whole percent) and the job we started | Rust serializes the state after each change, diffs it against the last published JSON (`json-patch`), converts to immer patches and emits `{seq, patches}`. Frontend applies with `applyPatches`; on a sequence gap it calls `get_state` for a full snapshot. |
| `telemetry` | positions, live feed, spindle rpm, temperatures | Emitted per status report (~4 Hz), replaces the previous value. Kept out of `AppState` so it doesn't churn patches. |
| `console` | console lines (sent, received, errors) | Append-only batches; history via `console_history`. Kept out of `AppState` because a bounded log diffs badly. |

Types shared with TypeScript are generated with `ts-rs` (`app/src/bindings/`).

## Command semantics

What we learned on hardware (`docs/research/z1-protocol.md`) is encoded in `hj-core`, not the UI:

- G-code lines are `ok`'d when queued; motion completion is judged from status (`Idle` + position).
- Several console lines at once are sent as a batch: one line, then the next after its `ok` (the
  machine holds the `ok` while its queue is full, so this is flow control), stopping at the first
  error, alarm or halt. Only G-code lines qualify: `$` and console commands don't all answer `ok`.
  Other motion is refused while a batch runs; Stop sending leaves queued moves to run.
- `$J` gets no reply; jogs queue. Stopping a jog means halt + `$X`.
- Continuous jogging (`JogHold`, a key or button held) streams short `$J` moves that keep about
  0.3 s of motion queued, ending each at the planned position's soft limit. It starts when idle,
  or while our own jogs are still running out (the core remembers where they end, and step jogs
  and moves to a point follow on from there too), so the planned position is exact. The frontend renews it every 150 ms and the core stops
  after 500 ms without, so a lost key-up can't run the machine into a limit. On release nothing
  more is queued and the machine stops smoothly within that lead; Esc halts at once.
- Halt (Ctrl-X) is the only immediate stop; `!` doesn't stop a move; `suspend`/`abort` drain the
  queue first.
- Positions in `Hold`/`Wait` status are planned end points, not live — telemetry marks them.
- Soft limits sit at the homed position (machine -1.0); never target it exactly.

## Gating

Every request is checked against the machine state before anything is sent: jog only when idle
(or while jogging), home when idle or alarmed, zero only when idle, halt always. Rejections come
back to the UI as errors with a reason; the UI also disables controls based on the same state, but
Rust is the authority.

## Programs

The program being edited lives in the frontend: CodeMirror holds the text, and a small store
(`app/src/lib/program.ts`) tracks the file, its name on the machine and whether it's dirty. That's
the one exception to "no state in React", made because the text changes on every keystroke and
the backend never needs it until upload. Everything else is in Rust:

- `open_program` / `save_program` read and write files (`hj_core::program`).
- `analyze_gcode` runs `hj-gcode` on the text (debounced after edits) and returns the toolpath and
  diagnostics. The result goes straight back to the caller, not into `AppState`: a toolpath is
  large and changes wholesale, which suits neither patches nor diffing. The frontend draws it on
  the bed at the current work offset and shows the diagnostics in the editor.
- `download_program` fetches a file from the SD card (`Core::download`, the mirror of an upload),
  passing the editor's text so the core can compare digests and skip the transfer if the
  machine's copy is the same. The editor remembers which machine file it's a copy of
  (`onMachine`); while that file runs as our job, the editor is read-only, highlights the current
  line and keeps it in view until the user scrolls away.
- `Upload { name, text, run }` is handled by the core: it drives the machine-pulled transfer
  (`hj_makera::transfer::Upload`) inside the actor, publishes progress, refreshes the listing and,
  with `run`, starts the job. The request resolves when the upload is done.

## Jobs and the SD card

- SD operations (`ls -s`, `rm`) answer in `LOAD_*` frames that don't say what they answer, so the
  core runs one at a time and rejects others meanwhile (`files.busy`). The listing of
  `/sd/gcodes` is fetched on connect and after uploads and deletes.
- Jobs only start from the editor, so it always shows what runs: an unchanged copy of a machine
  file runs that file (`play`), anything else is uploaded to `/sd/gcodes` first. Files on the SD
  card are opened in the editor (downloaded) to run them. The core records the job in
  `AppState.job` until the status stops reporting progress; a job started elsewhere only shows as
  `status.job`.
- Pause is `suspend` and Stop is `abort`; both let the machine finish its queued moves first (a
  few seconds), so the UI says so. After an abort the core sends `G90`, since `abort` leaves the
  job's modal state behind.
- Uploads and downloads are refused while a job runs, only one transfer runs at a time, and jobs
  can't start during a transfer.

## Tool changes

`ChangeTool` sends `M6 T<n>`; the machine goes to the change position and waits in the `Tool`
state, where `ConfirmToolChange` (`M490.2`) has it measure the new tool. The core tracks the
phase in `AppState.tool_change` (moving → waiting → measuring) also for changes a program or the
console started, from the `Tool` state and the target in the status. The state reads `Idle` for
moments during the routine, so only `Done ATC` (or an alarm) ends it, and other motion requests
are refused until then. The frontend shows a dialog for the whole change. The probes are tools
like any other: T9999 is the 3D probe, T0 the depth probe, T8888 the laser.

`RemeasureTool` measures the tool that's in again without a change (`M491`, the firmware's own
calibrate), for a tool that was taken out and put back. It shows as a change already measuring,
marked `remeasure`, and ends at `Done ATC` like one. With the 3D probe the firmware's script
switches the probe off at the end (a change to it leaves it on), so the core sends `M494.1` after.

## Outputs, overrides and the laser

- Overrides are `M220` (feed), `M223` (spindle) and `M325` (laser power), allowed during a job;
  the status report carries their values. The firmware applies them to moves queued after them.
- Outputs (light, power fan, spindle fan, external output, air) are switched with their M codes
  and read back from `diagnose`, which the core polls about once a second (not during transfers)
  into `AppState.outputs`. Only the outputs the report has are shown.
- The laser is a tool (T8888) like the probes. The firmware's laser mode follows it: a change to
  the laser is sent as `M321` (which does the tool change itself), a change away from it as `M322`
  then `M6`. Test fire (`M323`) needs the laser in; the UI turns it off when the window loses
  focus.

## Settings

`LoadSettings` downloads `/sd/config.txt` like any file (as the official controller does;
`config-get-all` printed nothing on the Z1), parses it as the firmware does, explains the keys it
knows (`hj_core::settings`) and publishes them in `AppState.settings`. Read-only for now.

## G-code reference

`hj_gcode::reference` is the one table of what each G/M code and console command does on the Z1,
with its parameters. `hj-gcode` uses it for "unknown code" diagnostics; the frontend fetches it
once (`gcode_reference`) for the editor's completion and hover help and for the console's, which
also completes console commands and parameter letters and shows the usage of the code being typed.

## Probing

Routines (`hj_core::probe`) run as a task beside the actor: they send lines through it (so they're
logged and ordered with everything else) and get the console lines forwarded while they run.
Every move is a `G38.x` probe move: searches are `G38.3` with the contact flag checked (a miss is
an error message, not a machine halt), releases `G38.5`, and positioning moves `G38.3` too, so an
unexpected contact stops the move and the routine. Touches are two-pass (fast, release, back
1 mm, slow). Routines start from wherever the user jogged the probe: an edge from beside the part
at the height to touch it (it just moves in), a bore from inside it, and a corner or boss from
above, where the routine finds the top itself before stepping out and down beside it. A bore or
boss can also be measured across one axis only, for the centreline and width of a slot or web. The
depth probe (T0) only touches down, so it runs Surface only, switched on with `M494.1` for the
routine and off after, as the firmware's own Z probing does. An
unexpected contact on a positioning move makes the routine back out to where that move started. Results are machine coordinates; applying one is `SetOrigin`,
a `G10 L20` relative to the current position, so nothing moves. Other motion is refused while a
routine runs; Stop is halt + `$X`.

### Surface maps

A surface map (`hj_core::map`) is a routine of its own: a grid of touches down, run point by
point in the same task slot (so Stop, Recover and the motion gating are shared), with its points
kept in `ProbeState.map` as they come in. The scan starts from where the probe is, which becomes
its top: moves never go above it, and each point searches down to a set depth below it. Between
points it lifts only the probe clearance above the last contact and crosses with a guarded move;
if the surface rises into the way, the 3D probe touches it sideways, releases, lifts another
clearance and carries on (never above the top). The depth probe can't feel sideways contact, so
with it every crossing is at the top. Rows alternate direction. A point with no contact within the
depth is recorded as missing and the scan goes on.

With refinement (`levels` > 0) the map lives on a finer lattice, the grid's spacing halved that
many times, of which only some points are probed: the grid first, then each cell's centre; where
the centre is further than the tolerance from its corners' average, or the corners and centre
don't all touch or all miss (an edge), the midpoints of its sides, then the same on its quarters.
`map::plan` works out the next batch from what's been measured, so the scan is a loop of plan and
probe, and resuming just plans again. The state keeps the points in the order they were probed,
and how many the current batch has left (more batches may follow). Slopes for ball compensation
come from a least-squares plane through the nearest points with contact (on a plain grid, central
differences), and the STL fans each cell from its centre round the points on its edge, so finer
neighbours leave no gaps.

The probe body feels nothing, so with the 3D probe each point also has its own floor: no deeper
than the stylus length (less 2 mm) below the highest point measured within the body's radius plus
one grid step. A narrow hole deeper than that is recorded as out of reach instead of driving the
body into its rim; crossings use the same floors at both ends. The depth from the start height
only says where to look for the surface, so a scan can start well above the part. Points not
measured yet can't count, so a wall taller than the stylus beside the path is still the user's to
avoid, as is anything above the start height within a body width of the area.

Pause asks the task to stop after the current point (at the top); Resume starts a new task from
the next point, going up to the top first; Stop halts as for any routine and keeps the points so
far. Export writes CSV, TSV or a binary STL of the grid in the current work coordinates. With ball
compensation, each point is moved from the ball's centre back by the radius along the normal of
the measured surface, estimated from its neighbours (the ball touches a slope off its bottom, so
the raw tip reads high there). The simulator has a sloped plate with a spherical dome for this.
The frontend has the same correction (`surfacePoints` in `app/src/lib/surfaceMap.ts`) for showing
the map, so what's shown matches what's exported; the two must be kept in step.

### Contours

A contour (`hj_core::contour`) traces a closed wall at the height the 3D probe was jogged to, in
the same routine slot: outside a part (from beside it, toward the middle of a box drawn around
it, which the trace mustn't leave) or inside a pocket (from within it, toward its −Y wall). It
walks round with the material on its left, so outside contours come out counter-clockwise and
inside ones clockwise. Each step stands off from the last touch by enough for the wall to bend
back toward the probe no tighter than the given radius (s² / 2(R − r) for the ball's centre, plus a
margin), steps along by the spacing and probes back toward the wall. A touch on the step along
means the wall bent in sooner; no touch means it bent away: the step is halved, retracing the path
each time, until it's within about a ball radius of the corner, then it swings back toward the last
touch (aimed a little into the material) to find the face round it. It's done back by the first
touch with a full turn behind it, and fails if it leaves the box or passes 5000 points. A trace that
stops partway (a halt, a refusal) keeps its touches; after Recover, Resume carries on from the last
two (their direction gives the wall's), at the same height. Points are
ball centres; compensation moves each a ball radius toward the material, square to the line through
its neighbours (`contour::wall`, mirrored in `app/src/lib/contour.ts`). Export is CSV/TSV or an R12
DXF with one closed polyline, in work coordinates, at the height the ball touched the wall.

### Transforming programs

There's no `G68` on the Z1, and no rotation kept anywhere as state: a part that sits turned is
dealt with by rotating the program's text (`hj_gcode::rotate`), so what's in the editor is what
runs, and the preview shows it lined up with what was probed. The work origin still carries the
translation. Rotating is all or nothing: if any line can't be rotated exactly (an arc outside the
XY plane, the A axis, G10/G92 in the program, probing moves, X/Y on G28/G30, or a one-axis move
before the other axis is known), nothing changes and those lines are listed. Otherwise only X/Y
and arc-centre I/J words change (moves get both X and Y; incremental moves and incremental arc
centres rotate as vectors), a comment at the top says what was done, and it's one undo step in the
editor. The editor's rotate bar shows the rotated program dashed on the map before it's applied.
The angle can come from the edge-angle routine: two touches `span` apart along an edge, giving how
far the part is turned (counter-clockwise positive), and "Rotate program by this" on its result.

Translating (`hj_gcode::transform::translate`) works the same way, by a relative X/Y/Z offset:
absolute positions move (X/Y/Z on moves, canned cycles' R and Z levels, absolute arc centres);
incremental values, G53 moves and arc radii stay; G10/G92, probing and G28/G30 by way of a position
are refused. Its main use is Z: a program whose Z0 is the stock's bottom, moved by minus the stock's
thickness, has its Z0 on the top. Both are cards in the editor's Transform sidebar; the card changed
last is previewed dashed in 2D, 3D and (for Z) on the Z bar.

### Fitting a drawing

A DXF's outlines (`hj_core::dxf`: lines, arcs, circles, polylines with bulges, blocks expanded,
units from `$INSUNITS` converted to mm, arcs as chords within 5 µm; pieces whose ends meet are
chained, closed outlines inside an odd number of others are holes; splines and the like are counted
as not read) can be fitted to a closed contour (`hj_core::fit`): point-to-line ICP of the traced ball
centres (raw) onto the picked outline offset by the ball's radius (outward round a part, into a
hole: exactly where the centres lie, arcs round outside corners included, with no per-point
estimate of the wall's direction as compensation needs), from 12 starting angles with the centres
together, coarse then fine, keeping distinct results so a symmetrical part offers its other fits.
The result is where the drawing's origin is (machine X/Y) and how it's turned. Using it sets the
work X/Y origin there and opens the Rotate card with the angle, so the program, drawn like the DXF,
is turned to match by rewriting it, as for any rotation. Reading and fitting are stateless Tauri
commands; the drawing, the picked outline and the fits live in the frontend (`lib/drawing.ts`),
which draws them on the 2D map: at the work origin before fitting, where the fit puts them after.

## The app around the machine

- **Busy.** `AppState::busy()` says what shouldn't be interrupted: a job, probing (maps and
  contours included), a tool change, a transfer or a console batch. The core tells the sink when
  it changes; the Tauri layer holds an idle-sleep assertion (`keepawake`) while it's set.
- **Quitting.** Closing the window or quitting (menu, dock) while busy or with unsaved edits is held
  in Rust and the window asks (`confirm-quit`, then `quit`). The window keeps Rust told about
  unsaved edits (`set_unsaved`). Quitting never stops the machine.
- **Notices.** The core reports a job finishing or stopping (aborted, alarm), a surface map or
  contour finishing or stopping, a tool change a job waits on, and the connection dropping while
  busy (`Sink::notice`). They show as OS notifications when the window isn't focused.
- **Reconnecting.** A connection that drops (rather than one closed with `Disconnect`) is tried
  again, 1 s then doubling to 10 s apart, for two minutes (`Connection::Reconnecting`); a job we
  started is followed again once back. `Connect` or `Disconnect` stops it. Only drops the
  transport reports are noticed: there is no reply timeout on the status polls.
- **Kept between runs.** The core keeps the probe settings and the last surface map and contour in
  `probing.json` in the app data folder (written a second after they change, whole). The window
  keeps its own settings in local storage (the probe form, jog step and speed,
  the map view, the last machine, recent files, panel sizes); the window-state plugin keeps the
  window's size and place.
- **Picked areas.** A surface map's area and an outside contour's box are kept in machine
  coordinates: they stay where they were drawn on the bed when the work origin moves. Their
  fields show them in work coordinates.
- **Markers.** Points to come back to, the window's alone (`lib/markers.ts`, in local storage):
  X/Y/Z (Z optional) kept in work coordinates (they move with the origin) or machine ones (they
  stay put), switchable. Added at the head, from a map click or from a probed point; drawn on both
  maps, named ones labelled; on the 2D map they get the same menu as probed points.
- **Menu.** Native, built in `src-tauri/src/menu.rs`; items only the window can do arrive there as
  a `menu` event with the item's id.
