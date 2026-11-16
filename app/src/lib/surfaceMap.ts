// Surface maps on the frontend: the grid the form describes, the lattice a map lives on, the
// surface it measured (compensated as in the export), and colours and stats. The scan itself runs
// in the core (`hj_core::map`), and this mirrors its geometry: keep them in step.
import type { MapSpec } from "@/bindings/MapSpec";
import type { SurfaceMap } from "@/bindings/SurfaceMap";
import type { ProbeForm } from "@/lib/ui";

export type Vec3 = [number, number, number];

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
    levels: form.mapAdaptive ? form.mapLevels : 0,
    tolerance: form.mapAdaptive ? form.mapTolerance : 0,
  };
}

/** Lattice points per grid step. */
export const scaleOf = (spec: MapSpec) => 2 ** spec.levels;

/** Columns and rows of the lattice (the grid when not refined). */
export function lattice(spec: MapSpec): [number, number] {
  const s = scaleOf(spec);
  return [(spec.columns - 1) * s + 1, (spec.rows - 1) * s + 1];
}

/** The lattice's spacing along X and Y, mm. */
export function stepOf(spec: MapSpec): [number, number] {
  const [cols, rows] = lattice(spec);
  return [(spec.max[0] - spec.min[0]) / (cols - 1), (spec.max[1] - spec.min[1]) / (rows - 1)];
}

/** Machine X/Y of lattice point `i` (row by row from the front, each from the left). */
export function pointAt(spec: MapSpec, i: number): [number, number] {
  const [cols] = lattice(spec);
  const [dx, dy] = stepOf(spec);
  return [spec.min[0] + dx * (i % cols), spec.min[1] + dy * Math.floor(i / cols)];
}

/** Whether lattice point `i` is on the (unrefined) grid. */
export function onGrid(spec: MapSpec, i: number) {
  const [cols] = lattice(spec);
  const s = scaleOf(spec);
  return (i % cols) % s === 0 && Math.floor(i / cols) % s === 0;
}

/** Probed or not, per lattice point. */
export function probedMask(map: SurfaceMap): boolean[] {
  const out = new Array<boolean>(map.z.length).fill(false);
  for (const i of map.probed) out[i] = true;
  return out;
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

/** Time for the points planned, from the average so far (refinement may plan more after). */
export function remainingSecs(map: SurfaceMap) {
  const done = map.probed.length;
  return done > 0 ? (map.secs / done) * map.pending : null;
}

export function duration(secs: number) {
  if (secs < 90) return `${Math.round(secs)} s`;
  const m = Math.round(secs / 60);
  return m < 90 ? `${m} min` : `${Math.floor(m / 60)} h ${m % 60} min`;
}

/** Lattice points within `radius` (mm) of lattice point `i`, itself included. */
function within(spec: MapSpec, i: number, radius: number): number[] {
  const [cols, rows] = lattice(spec);
  const [dx, dy] = stepOf(spec);
  const dc = Math.min(Math.floor(radius / dx), cols);
  const dr = Math.min(Math.floor(radius / dy), rows);
  const c = i % cols;
  const r = Math.floor(i / cols);
  const out: number[] = [];
  for (let rr = Math.max(r - dr, 0); rr < Math.min(r + dr + 1, rows); rr++) {
    for (let cc = Math.max(c - dc, 0); cc < Math.min(c + dc + 1, cols); cc++) {
      if (Math.hypot((cc - c) * dx, (rr - r) * dy) <= radius + 1e-9) out.push(rr * cols + cc);
    }
  }
  return out;
}

/**
 * The measured surface's slope (dz/dx, dz/dy) at lattice point `i`: the least-squares plane
 * through it and the nearest points with contact, within its own level's spacing, or further out
 * where those don't fix a plane. Mirrors `hj_core::map::slope`.
 */
function slope(spec: MapSpec, z: (number | null)[], i: number): [number, number] {
  const [cols] = lattice(spec);
  const c = i % cols;
  const r = Math.floor(i / cols);
  const [dx, dy] = stepOf(spec);
  const h = z[i] ?? 0;
  const scale = scaleOf(spec);
  let p = 1;
  while (p < scale && c % (p * 2) === 0 && r % (p * 2) === 0) p *= 2;
  const grid = scale * Math.max(dx, dy);
  let radius = p * Math.max(dx, dy) * 1.01;
  let sums = [0, 0, 0, 0, 0];
  while (radius <= grid * 2) {
    sums = [0, 0, 0, 0, 0];
    for (const j of within(spec, i, radius)) {
      const zj = z[j];
      if (j === i || zj === null) continue;
      const ox = ((j % cols) - c) * dx;
      const oy = (Math.floor(j / cols) - r) * dy;
      const oz = zj - h;
      sums = [sums[0] + ox * ox, sums[1] + ox * oy, sums[2] + oy * oy, sums[3] + ox * oz, sums[4] + oy * oz];
    }
    const det = sums[0] * sums[2] - sums[1] * sums[1];
    if (det > 1e-9 * (sums[0] + sums[2]) ** 2 && sums[0] > 0 && sums[2] > 0) {
      return [(sums[3] * sums[2] - sums[4] * sums[1]) / det, (sums[4] * sums[0] - sums[3] * sums[1]) / det];
    }
    radius *= Math.SQRT2;
  }
  return [sums[0] > 0 ? sums[3] / sums[0] : 0, sums[2] > 0 ? sums[4] / sums[2] : 0];
}

/**
 * The measured surface at each lattice point, machine coordinates; null where there's no contact
 * (or no point). With `compensate`, corrected for the ball touching a slope off its bottom: each
 * point is moved from the ball's centre back by the radius along the surface's normal. Mirrors
 * `hj_core::map::surface`, which the export uses.
 */
export function surfacePoints(map: SurfaceMap, ballRadius: number, compensate: boolean): (Vec3 | null)[] {
  const { spec } = map;
  return map.z.map((h, i) => {
    if (h === null) return null;
    const [x, y] = pointAt(spec, i);
    if (!compensate) return [x, y, h];
    const [gx, gy] = slope(spec, map.z, i);
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
 * The probed lattice point nearest machine (x, y) within `radius` (mm), or null. Looks only at the
 * lattice around it (compensation moves points by less than a ball radius).
 */
export function nearestPoint(map: SurfaceMap, probed: boolean[], points: (Vec3 | null)[], x: number, y: number, radius: number) {
  const { spec } = map;
  const [cols, rows] = lattice(spec);
  const [dx, dy] = stepOf(spec);
  const fx = (x - spec.min[0]) / dx;
  const fy = (y - spec.min[1]) / dy;
  // Far enough to cover the radius, but not the whole lattice on a dense map.
  const reach = (d: number) => Math.min(Math.ceil(radius / d) + 1, 60);
  let best: number | null = null;
  let bestD = radius;
  for (let r = Math.max(Math.floor(fy) - reach(dy), 0); r <= Math.min(Math.ceil(fy) + reach(dy), rows - 1); r++) {
    for (let c = Math.max(Math.floor(fx) - reach(dx), 0); c <= Math.min(Math.ceil(fx) + reach(dx), cols - 1); c++) {
      const i = r * cols + c;
      if (!probed[i]) continue;
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

/**
 * The cells a probed lattice makes, each as the probed points around its edge (counter-clockwise
 * from its front-left corner): a cell is split in four where its sides' midpoints were probed.
 * Mirrors `hj_core::map::leaves` and `rim`; the 3D view draws them.
 */
export function cellRims(spec: MapSpec, probed: boolean[]): number[][] {
  const [cols, rows] = lattice(spec);
  const s = scaleOf(spec);
  const at = (c: number, r: number) => r * cols + c;
  const out: number[][] = [];
  const walk = (c0: number, r0: number, size: number) => {
    const h = size / 2;
    const split = size >= 2 && [at(c0 + h, r0), at(c0 + size, r0 + h), at(c0 + h, r0 + size), at(c0, r0 + h)].every((i) => probed[i]);
    if (split) {
      for (const [c, r] of [
        [c0, r0],
        [c0 + h, r0],
        [c0, r0 + h],
        [c0 + h, r0 + h],
      ])
        walk(c, r, h);
      return;
    }
    const rim: number[] = [];
    for (let k = 0; k < size; k++) rim.push(at(c0 + k, r0));
    for (let k = 0; k < size; k++) rim.push(at(c0 + size, r0 + k));
    for (let k = 0; k < size; k++) rim.push(at(c0 + size - k, r0 + size));
    for (let k = 0; k < size; k++) rim.push(at(c0, r0 + size - k));
    out.push(rim.filter((i) => probed[i]));
  };
  for (let r0 = 0; r0 < rows - 1; r0 += s) for (let c0 = 0; c0 < cols - 1; c0 += s) walk(c0, r0, s);
  return out;
}
