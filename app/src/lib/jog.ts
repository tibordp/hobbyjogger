// Tap to step, hold to jog continuously, for keys and buttons alike. A press that ends within
// HOLD_AFTER is a tap (one step); one that lasts longer turns into a held jog, which the core runs
// while it keeps hearing from us (see `Request::JogHold`).
import { create } from "zustand";

import { request } from "@/lib/backend";
import { act } from "@/lib/actions";
import { useUi } from "@/lib/ui";

export type Dir = { x?: number; y?: number; z?: number; a?: number };

/** How long a press lasts before it's a hold rather than a tap, ms. */
const HOLD_AFTER = 250;
/** How often a held jog is renewed; the core stops after 500 ms without. */
const RENEW = 150;

interface Press {
  dir: Dir;
  tap: () => void;
  holding: boolean;
  /** The held jog failed (refused, say): releasing it does nothing. */
  dead: boolean;
  timer: ReturnType<typeof setTimeout>;
}

const presses = new Map<string, Press>();

/** A direction as a key, e.g. "x-1". */
export const dirKey = (d: Dir) =>
  (["x", "y", "z", "a"] as const)
    .filter((k) => d[k])
    .map((k) => `${k}${Math.sign(d[k]!)}`)
    .join(" ");

/** The directions being pressed and those moving continuously, so the buttons can show them. */
export const useJogPresses = create<{ pressed: string[]; held: string[] }>(() => ({ pressed: [], held: [] }));

function publish() {
  const live = [...presses.values()].filter((p) => !p.dead);
  useJogPresses.setState({
    pressed: live.map((p) => dirKey(p.dir)),
    held: live.filter((p) => p.holding).map((p) => dirKey(p.dir)),
  });
}
let renewal: ReturnType<typeof setInterval> | null = null;
/** Requests go out one after another, so a renewal can't overtake the release and restart it. */
let chain: Promise<unknown> = Promise.resolve();
const enqueue = (f: () => Promise<unknown>) => (chain = chain.then(f));

/** The held presses' directions combined, e.g. → and ↑ for a diagonal. */
function heldDir(): Dir | null {
  const sum = { x: 0, y: 0, z: 0, a: 0 };
  for (const p of presses.values()) {
    if (!p.holding) continue;
    for (const k of ["x", "y", "z", "a"] as const) sum[k] += p.dir[k] ?? 0;
  }
  const dir: Dir = {};
  for (const k of ["x", "y", "z", "a"] as const) if (sum[k] !== 0) dir[k] = Math.sign(sum[k]);
  return Object.keys(dir).length ? dir : null;
}

async function send(first: boolean) {
  const dir = heldDir();
  // Released meanwhile.
  if (!dir || !renewal) return;
  const r = { type: "jogHold" as const, ...dir, speed: useUi.getState().jogSpeed };
  // The first one says why if it's refused; renewals fail quietly (the machine halted, say).
  const ok = first ? await act(r) : await request(r).then(() => true, () => false);
  if (!ok) {
    for (const p of presses.values()) if (p.holding) Object.assign(p, { holding: false, dead: true });
    update();
  }
}

/** Start, steer or end the held jog to match the presses. */
function update() {
  const holding = heldDir() !== null;
  queueMicrotask(publish);
  if (holding && !renewal) {
    renewal = setInterval(() => void enqueue(() => send(false)), RENEW);
    void enqueue(() => send(true));
  } else if (holding) {
    void enqueue(() => send(false));
  } else if (renewal) {
    clearInterval(renewal);
    renewal = null;
    void enqueue(() => act({ type: "jogRelease" }));
  }
}

/** A key or button went down: `source` names it, `tap` makes the single step. */
export function press(source: string, dir: Dir, tap: () => void) {
  if (presses.has(source)) return;
  const p: Press = {
    dir,
    tap,
    holding: false,
    dead: false,
    timer: setTimeout(() => {
      p.holding = true;
      update();
    }, HOLD_AFTER),
  };
  presses.set(source, p);
  publish();
}

export function release(source: string) {
  const p = presses.get(source);
  if (!p) return;
  clearTimeout(p.timer);
  presses.delete(source);
  // A tap while another key holds a jog would only be refused: the hold is the gesture.
  if (!p.holding && !p.dead && !renewal) p.tap();
  update();
}

/** Drop every press without stepping, e.g. when the window loses focus. */
export function releaseAll() {
  for (const p of presses.values()) clearTimeout(p.timer);
  presses.clear();
  update();
}
