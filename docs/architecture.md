# Architecture

Hobby Jogger is a Tauri app. Rust owns the machine connection, all application state and every
decision about what may be sent to the machine. The React frontend renders that state and turns
user input into requests; it holds no state of its own beyond view concerns (open panels, input
fields).

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
- **`hj-sim`** — a simulated Z1 (TCP, framed protocol) implementing the behaviour we measured on
  the real machine: status, `$J`/`$H`, `G53` moves, halt/`$X`, soft limits, `G10 L20`, version/model.
  Used by tests and by the app's "Simulator" connection.
- **`hj-cli`** — the `hj` protocol exploration tool.
- **`app/src-tauri`** — the Tauri binary: registers commands that forward to `hj-core` and
  implements `Sink` by emitting events.

## State sync

Three channels, chosen by how the data changes:

| Channel | Content | Mechanism |
|---|---|---|
| `state-sync` | `AppState`: connection, machine info, slow-changing status (state, halt, tool, spindle target, offsets, job), discovered machines | Rust serializes the state after each change, diffs it against the last published JSON (`json-patch`), converts to immer patches and emits `{seq, patches}`. Frontend applies with `applyPatches`; on a sequence gap it calls `get_state` for a full snapshot. |
| `telemetry` | positions, live feed, spindle rpm, temperatures | Emitted per status report (~4 Hz), replaces the previous value. Kept out of `AppState` so it doesn't churn patches. |
| `console` | console lines (sent, received, errors) | Append-only batches; history via `console_history`. Kept out of `AppState` because a bounded log diffs badly. |

Types shared with TypeScript are generated with `ts-rs` (`app/src/bindings/`).

## Command semantics

What we learned on hardware (`docs/research/z1-protocol.md`) is encoded in `hj-core`, not the UI:

- G-code lines are `ok`'d when queued; motion completion is judged from status (`Idle` + position).
- `$J` gets no reply; jogs queue. Stopping a jog means halt + `$X`.
- Halt (Ctrl-X) is the only immediate stop; `!` doesn't stop a move; `suspend`/`abort` drain the
  queue first.
- Positions in `Hold`/`Wait` status are planned end points, not live — telemetry marks them.
- Soft limits sit at the homed position (machine -1.0); never target it exactly.

## Gating

Every request is checked against the machine state before anything is sent: jog only when idle
(or while jogging), home when idle or alarmed, zero only when idle, halt always. Rejections come
back to the UI as errors with a reason; the UI also disables controls based on the same state, but
Rust is the authority.
