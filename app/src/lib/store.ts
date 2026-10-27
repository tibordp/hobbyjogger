// Mirrors of the Rust-side state. `state` is kept in sync with immer patches (with a full resync
// whenever a sequence number is missed); telemetry and console arrive on their own channels.
import { applyPatches, enablePatches, type Patch as ImmerPatch } from "immer";
import { create } from "zustand";

import type { AppState } from "@/bindings/AppState";
import type { ConsoleEntry } from "@/bindings/ConsoleEntry";
import type { StateUpdate } from "@/bindings/StateUpdate";
import type { Telemetry } from "@/bindings/Telemetry";
import { consoleHistory, getState, onConsole, onStateSync, onTelemetry } from "@/lib/backend";

enablePatches();

const CONSOLE_LIMIT = 2000;

interface Store {
  seq: number;
  state: AppState | null;
  telemetry: Telemetry | null;
  console: ConsoleEntry[];
}

export const useStore = create<Store>(() => ({
  seq: -1,
  state: null,
  telemetry: null,
  console: [],
}));

/** Updates that arrive while a snapshot is being fetched; `null` when not resyncing. */
let queued: StateUpdate[] | null = null;

function onUpdate(update: StateUpdate) {
  if (queued) {
    queued.push(update);
    return;
  }
  const { seq, state } = useStore.getState();
  if (update.seq <= seq) return;
  if (state === null || update.seq !== seq + 1) {
    void resync();
    return;
  }
  // Rust paths are (string | number)[] like immer's; the shapes match by construction.
  const next = applyPatches(state, update.patches as ImmerPatch[]);
  useStore.setState({ seq: update.seq, state: next });
}

async function resync() {
  if (queued) return;
  queued = [];
  try {
    const snapshot = await getState();
    useStore.setState({ seq: snapshot.seq, state: snapshot.state });
  } finally {
    const pending = queued;
    queued = null;
    for (const u of pending) onUpdate(u);
  }
}

function appendConsole(entries: ConsoleEntry[]) {
  useStore.setState((s) => {
    const last = s.console.at(-1)?.id ?? 0;
    // Live entries normally arrive in order; history fetched at startup may overlap them.
    const merged = entries.every((e) => e.id > last)
      ? [...s.console, ...entries]
      : [...new Map([...s.console, ...entries].map((e) => [e.id, e])).values()].sort(
          (a, b) => a.id - b.id,
        );
    return { console: merged.slice(-CONSOLE_LIMIT) };
  });
}

let started = false;

/** Subscribe to the backend and load the initial state. Call once at startup. */
export async function startSync() {
  if (started) return;
  started = true;
  // Subscribe before fetching so nothing is missed in between.
  await Promise.all([
    onStateSync(onUpdate),
    onTelemetry((telemetry) => useStore.setState({ telemetry })),
    onConsole(appendConsole),
  ]);
  await resync();
  appendConsole(await consoleHistory());
}
