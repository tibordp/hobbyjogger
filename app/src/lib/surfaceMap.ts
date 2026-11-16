// Surface maps on the frontend: the grid the form describes, colours for heights, and stats.
// The scan itself runs in the core (`hj_core::map`).
import type { MapSpec } from "@/bindings/MapSpec";
import type { SurfaceMap } from "@/bindings/SurfaceMap";
import type { ProbeForm } from "@/lib/ui";

/** Points along one side: as close to `spacing` as fits the length evenly, at least 2. */
export function count(length: number, spacing: number) {
  return Math.max(2, Math.round(length / spacing) + 1);
}

/** The grid the form describes, in machine coordinates (`offset` is machine − work). */
export function plannedSpec(form: ProbeForm, offset: [number, number] | null): MapSpec | null {
  if (!form.mapMin || !form.mapMax || !offset) return null;
  const [x0, y0] = form.mapMin;
  const [x1, y1] = form.mapMax;
  if (!(x1 > x0 && y1 > y0)) return null;
  return {
    min: [x0 + offset[0], y0 + offset[1]],
    max: [x1 + offset[0], y1 + offset[1]],
    columns: count(x1 - x0, form.mapSpacing),
    rows: count(y1 - y0, form.mapSpacing),
    depth: form.mapDepth,
    precise: form.mapPrecise,
  };
}

/** Machine X/Y of grid point `i` (row by row from the front, each from the left). */
export function pointAt(spec: MapSpec, i: number): [number, number] {
  const c = i % spec.columns;
  const r = Math.floor(i / spec.columns);
  return [
    spec.min[0] + ((spec.max[0] - spec.min[0]) * c) / (spec.columns - 1),
    spec.min[1] + ((spec.max[1] - spec.min[1]) * r) / (spec.rows - 1),
  ];
}

export function zRange(map: SurfaceMap): [number, number] | null {
  let lo = Infinity;
  let hi = -Infinity;
  for (const z of map.z) {
    if (z === null) continue;
    lo = Math.min(lo, z);
    hi = Math.max(hi, z);
  }
  return lo <= hi ? [lo, hi] : null;
}

/** Low to high: blue, teal, yellow, red. `t` in [0, 1]. As [r, g, b] in 0–1. */
export function heat(t: number): [number, number, number] {
  const stops: [number, number, number][] = [
    [0.23, 0.51, 0.96],
    [0.08, 0.72, 0.65],
    [0.98, 0.8, 0.08],
    [0.94, 0.27, 0.27],
  ];
  const x = Math.min(Math.max(t, 0), 1) * (stops.length - 1);
  const i = Math.min(Math.floor(x), stops.length - 2);
  const f = x - i;
  return [0, 1, 2].map((k) => stops[i][k] + (stops[i + 1][k] - stops[i][k]) * f) as [number, number, number];
}

export function heatCss(t: number) {
  const [r, g, b] = heat(t);
  return `rgb(${Math.round(r * 255)} ${Math.round(g * 255)} ${Math.round(b * 255)})`;
}

/** Time left, from the average so far. */
export function remainingSecs(map: SurfaceMap) {
  const total = map.spec.columns * map.spec.rows;
  return map.done > 0 ? (map.secs / map.done) * (total - map.done) : null;
}

export function duration(secs: number) {
  if (secs < 90) return `${Math.round(secs)} s`;
  const m = Math.round(secs / 60);
  return m < 90 ? `${m} min` : `${Math.floor(m / 60)} h ${m % 60} min`;
}

export type Vec3 = [number, number, number];

/**
 * The measured surface at each grid point, machine coordinates; null where there's no point. With
 * `compensate`, corrected for the ball touching a slope off its bottom: each point is moved from
 * the ball's centre back by the radius along the normal of the measured surface, estimated from
 * its neighbours. Mirrors `hj_core::map::surface`, which the export uses; keep them in step.
 */
export function surfacePoints(map: SurfaceMap, ballRadius: number, compensate: boolean): (Vec3 | null)[] {
  const { spec } = map;
  const cols = spec.columns;
  const rows = spec.rows;
  const z = (c: number, r: number) => map.z[r * cols + c];
  const dx = (spec.max[0] - spec.min[0]) / (cols - 1);
  const dy = (spec.max[1] - spec.min[1]) / (rows - 1);
  const slope = (before: number | null, here: number, after: number | null, step: number) =>
    before !== null && after !== null
      ? (after - before) / (2 * step)
      : before !== null
        ? (here - before) / step
        : after !== null
          ? (after - here) / step
          : 0;
  return map.z.map((h, i) => {
    if (h === null) return null;
    const [x, y] = pointAt(spec, i);
    if (!compensate) return [x, y, h];
    const c = i % cols;
    const r = Math.floor(i / cols);
    const gx = slope(c > 0 ? z(c - 1, r) : null, h, c + 1 < cols ? z(c + 1, r) : null, dx);
    const gy = slope(r > 0 ? z(c, r - 1) : null, h, r + 1 < rows ? z(c, r + 1) : null, dy);
    const len = Math.sqrt(gx * gx + gy * gy + 1);
    return [x + (ballRadius * gx) / len, y + (ballRadius * gy) / len, h + ballRadius - ballRadius / len];
  });
}

/** Lowest and highest Z of `points`, or null without any. */
export function pointsRange(points: (Vec3 | null)[]): [number, number] | null {
  let lo = Infinity;
  let hi = -Infinity;
  for (const p of points) {
    if (!p) continue;
    lo = Math.min(lo, p[2]);
    hi = Math.max(hi, p[2]);
  }
  return lo <= hi ? [lo, hi] : null;
}

/**
 * The grid point nearest machine (x, y) within `radius`, among those with a surface point. Looks
 * only at the grid cells around it: compensation moves points by less than a ball radius.
 */
export function nearestPoint(spec: MapSpec, points: (Vec3 | null)[], x: number, y: number, radius: number) {
  const fx = ((x - spec.min[0]) / (spec.max[0] - spec.min[0])) * (spec.columns - 1);
  const fy = ((y - spec.min[1]) / (spec.max[1] - spec.min[1])) * (spec.rows - 1);
  let best: number | null = null;
  let bestD = radius;
  for (let r = Math.floor(fy) - 1; r <= Math.ceil(fy) + 1; r++) {
    for (let c = Math.floor(fx) - 1; c <= Math.ceil(fx) + 1; c++) {
      if (r < 0 || c < 0 || r >= spec.rows || c >= spec.columns) continue;
      const i = r * spec.columns + c;
      const [px, py] = points[i] ?? pointAt(spec, i);
      const d = Math.hypot(px - x, py - y);
      if (d <= bestD) {
        best = i;
        bestD = d;
      }
    }
  }
  return best;
}

/** Whether grid point `i` has been probed (rows go in alternating directions). */
export function isDone(map: SurfaceMap, i: number) {
  const cols = map.spec.columns;
  const r = Math.floor(i / cols);
  const c = i % cols;
  return r * cols + (r % 2 === 0 ? c : cols - 1 - c) < map.done;
}
