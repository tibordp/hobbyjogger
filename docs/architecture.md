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
