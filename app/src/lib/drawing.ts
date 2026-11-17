// A reference drawing (DXF) for fitting to a traced contour: its outlines, the one picked, and
// how it fits. Reading and fitting happen in Rust (`hj_core::dxf`, `hj_core::fit`); this keeps the
// result and places it on the bed.
import { open } from "@tauri-apps/plugin-dialog";
import { toast } from "sonner";
import { create } from "zustand";

import type { Drawing } from "@/bindings/Drawing";
import type { Fit } from "@/bindings/Fit";
import { fitOutline, importDxf } from "@/lib/backend";

export type V2 = [number, number];

interface DrawingState {
  name: string;
  drawing: Drawing;
  /** The outline picked to fit. */
  selected: number | null;
  /** The ways it fits the contour, best first; null until fitted. */
  fits: Fit[] | null;
  /** Which of `fits` is shown. */
  shown: number;
  fitting: boolean;
}

export const useDrawing = create<{ current: DrawingState | null }>(() => ({ current: null }));

const set = (patch: Partial<DrawingState>) =>
  useDrawing.setState((s) => (s.current ? { current: { ...s.current, ...patch } } : {}));

export async function openDrawing() {
  const path = await open({ filters: [{ name: "DXF drawing", extensions: ["dxf"] }] });
  if (typeof path !== "string") return;
  try {
    const drawing = await importDxf(path);
    const closed = drawing.outlines.filter((o) => o.closed);
    if (closed.length === 0) {
      toast.error("That drawing has no closed outlines to fit");
      return;
    }
    // The largest is usually the part's outside.
    const largest = drawing.outlines.indexOf(closed.reduce((a, b) => (b.area > a.area ? b : a)));
    useDrawing.setState({
      current: { name: path.split(/[\\/]/).pop() ?? path, drawing, selected: largest, fits: null, shown: 0, fitting: false },
    });
  } catch (e) {
    toast.error(String(e));
  }
}

export const clearDrawing = () => useDrawing.setState({ current: null });

/** Pick an outline (closed ones only); a new pick forgets the fit. */
export function selectOutline(i: number) {
  const c = useDrawing.getState().current;
  if (c?.drawing.outlines[i]?.closed && c.selected !== i) set({ selected: i, fits: null, shown: 0 });
}

export const showFit = (shown: number) => set({ shown });

/** Fit the picked outline to `measured` (where the ball's centre was at each touch, machine X/Y),
 * which lie `offset` (the ball's radius, negative inside a hole) off the wall. */
export async function runFit(measured: V2[], offset: number) {
  const c = useDrawing.getState().current;
  if (!c || c.selected === null) return;
  set({ fitting: true });
  try {
    const fits = await fitOutline(c.drawing.outlines[c.selected].points as V2[], measured, offset);
    set({ fits, shown: 0 });
    if (fits.length === 0) toast.error("Couldn't fit it");
  } finally {
    set({ fitting: false });
  }
}

/**
 * Drawing → machine X/Y: by the fit shown, or before fitting, with the drawing's origin on the
 * work origin (`offset`, machine − work), as a program would be.
 */
export function placement(c: DrawingState, offset: V2 | null): ((p: V2) => V2) | null {
  const fit = c.fits?.[c.shown];
  if (fit) {
    const [s, co] = [Math.sin((fit.angle * Math.PI) / 180), Math.cos((fit.angle * Math.PI) / 180)];
    return ([x, y]) => [x * co - y * s + fit.origin[0], x * s + y * co + fit.origin[1]];
  }
  if (!offset) return null;
  return ([x, y]) => [x + offset[0], y + offset[1]];
}

/** Distance from `p` to the polyline `pts` (closed or not). */
export function distanceTo(pts: V2[], closed: boolean, p: V2) {
  let best = Infinity;
  const n = closed ? pts.length : pts.length - 1;
  for (let i = 0; i < n; i++) {
    const [a, b] = [pts[i], pts[(i + 1) % pts.length]];
    const [dx, dy] = [b[0] - a[0], b[1] - a[1]];
    const l2 = dx * dx + dy * dy;
    const u = l2 > 0 ? Math.min(Math.max(((p[0] - a[0]) * dx + (p[1] - a[1]) * dy) / l2, 0), 1) : 0;
    best = Math.min(best, Math.hypot(a[0] + u * dx - p[0], a[1] + u * dy - p[1]));
  }
  return best;
}
