import { Crosshair, Focus, MapPin, Move, Trash2, TriangleAlert } from "lucide-react";
import { lazy, Suspense, useEffect, useMemo, useRef, useState } from "react";

import type { Analysis } from "@/bindings/Analysis";
import type { Corner } from "@/bindings/Corner";
import type { MapSpec } from "@/bindings/MapSpec";
import type { ProbeState } from "@/bindings/ProbeState";
import type { SurfaceMap } from "@/bindings/SurfaceMap";
import type { Side } from "@/bindings/Side";
import { Button } from "@/components/ui/button";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuLabel,
  DropdownMenuSeparator,
  DropdownMenuShortcut,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import { coordsText, PointActions } from "@/components/PointActions";
import { ZBar } from "@/components/ZBar";
import { act } from "@/lib/actions";
import { plannedContour, wallPoints, type V2 } from "@/lib/contour";
import { distanceTo, placement, selectOutline, useDrawing } from "@/lib/drawing";
import { canJog, isProbe, useConnected, useJobRunning, useMachineState, useStatus, useWorkOffset } from "@/lib/machine";
import { addMarker, machinePos, markerLabel, removeMarker, useMarkers } from "@/lib/markers";
import { fitProblems, useFollowing, useProgram } from "@/lib/program";
import { useStore } from "@/lib/store";
import { heatCss, lattice, nearestPoint, onGrid, plannedSpec, pointAt, pointsRange, probedMask, scaleOf, surfacePoints, type Vec3 as MapPoint } from "@/lib/surfaceMap";
import { previewOf } from "@/lib/transform";
import { areaOf, type ProbeForm, setMapView, setProbeForm, useUi } from "@/lib/ui";
import { cn } from "@/lib/utils";

const Bed3D = lazy(() => import("@/components/Bed3D"));

/** Used until the machine reports its limits (Z1 values). */
const DEFAULT_MIN = [-207, -206];
/** Room around the bed at Fit, mm. */
const MARGIN = 8;
/** How far the 2D map zooms in, relative to the whole bed. */
const MAX_ZOOM = 1000;
/** Grid lines closer than this aren't drawn, px. */
const GRID_MIN_PX = 7;
/** Above the top and below the bottom of the Z bar's track, px. */
const Z_BAR_PAD = 10;

type Point = { x: number; y: number };

const JUMP = 0;
const RAPID = 1;

/** First vertex index whose source line is after `line` (lines never decrease). */
function vertexAfterLine(a: Analysis, line: number) {
  let lo = 0;
  let hi = a.lines.length;
  while (lo < hi) {
    const mid = (lo + hi) >> 1;
    if (a.lines[mid] <= line) lo = mid + 1;
    else hi = mid;
  }
  return lo;
}

/**
 * SVG path data for the segments ending at vertices `[from, to)`, split into rapids and feeds.
 * `map` turns work XY into SVG coordinates.
 */
function pathData(a: Analysis, from: number, to: number, map: (x: number, y: number) => [number, number]) {
  const out = { rapid: [] as string[], feed: [] as string[] };
  const ends = { rapid: -1, feed: -1 };
  const pt = (i: number) => {
    const [x, y] = map(a.points[i * 3], a.points[i * 3 + 1]);
    return `${x.toFixed(3)} ${y.toFixed(3)}`;
  };
  for (let i = Math.max(from, 1); i < to; i++) {
    const kind = a.kinds[i];
    if (kind === JUMP) continue;
    const key = kind === RAPID ? "rapid" : "feed";
    if (ends[key] !== i - 1) out[key].push(`M${pt(i - 1)}`);
    out[key].push(`L${pt(i)}`);
    ends[key] = i;
  }
  return { rapid: out.rapid.join(""), feed: out.feed.join("") };
}

/** A map point (lattice index), a contour point (index round it), a marker (index in the list) or
 * the work origin, for hover and its menu. */
type Target = { kind: "map" | "contour" | "marker" | "origin"; i: number };

/** Machine X or Y to the drawing's units (mm). */
type ToSvg = (v: number) => number;
// `k` below is the drawing's units per screen pixel: sizes written `n * k` are n pixels on screen
// at any zoom.

/** Contacts of the current or last probing routine, and what it found. */
function ProbeOverlay({ probe, sx, sy, w, h, k }: { probe: ProbeState; sx: ToSvg; sy: ToSvg; w: number; h: number; k: number }) {
  const r = probe.result;
  return (
    <g className="pointer-events-none">
      {probe.contacts.map((c, i) => (
        <circle key={i} cx={sx(c[0])} cy={sy(c[1])} r={2.5 * k} className="fill-sky-300" />
      ))}
      {r && !probe.running && (
        <g className="fill-none stroke-sky-300" strokeWidth={1.25 * k}>
          {r.x !== null && r.y === null && <line x1={sx(r.x)} y1={0} x2={sx(r.x)} y2={h} strokeDasharray={`${5 * k} ${4 * k}`} />}
          {r.y !== null && r.x === null && <line x1={0} y1={sy(r.y)} x2={w} y2={sy(r.y)} strokeDasharray={`${5 * k} ${4 * k}`} />}
          {r.x !== null && r.y !== null && (
            <>
              <line x1={sx(r.x) - 12 * k} y1={sy(r.y)} x2={sx(r.x) + 12 * k} y2={sy(r.y)} />
              <line x1={sx(r.x)} y1={sy(r.y) - 12 * k} x2={sx(r.x)} y2={sy(r.y) + 12 * k} />
              {r.diameter !== null && <circle cx={sx(r.x)} cy={sy(r.y)} r={r.diameter / 2} />}
            </>
          )}
        </g>
      )}
    </g>
  );
}

/** The area of a planned map, and its points (dim) when `dots` and there are few enough to see. */
function PlannedMap({ spec, dots, sx, sy, k }: { spec: MapSpec; dots: boolean; sx: ToSvg; sy: ToSvg; k: number }) {
  const n = spec.columns * spec.rows;
  const [x0, y0] = [sx(spec.min[0]), sy(spec.max[1])];
  return (
    <g className="pointer-events-none">
      <rect x={x0} y={y0} width={sx(spec.max[0]) - x0} height={sy(spec.min[1]) - y0} className="fill-sky-400/5 stroke-sky-400/70" strokeWidth={k} strokeDasharray={`${4 * k} ${3 * k}`} />
      {dots &&
        n <= 4000 &&
        Array.from({ length: n }, (_, i) => {
          const [x, y] = pointAt(spec, i);
          return <circle key={i} cx={sx(x)} cy={sy(y)} r={1.25 * k} className="fill-sky-400/60" />;
        })}
    </g>
  );
}

/** A measured (or partly measured) map: `points` (the surface, compensated or not) coloured from
 * low to high; misses as grey rings, points out of the stylus' reach as amber ones. */
function MeasuredMap({
  map,
  probed,
  points,
  sx,
  sy,
  k,
}: {
  map: SurfaceMap;
  probed: boolean[];
  points: (MapPoint | null)[];
  sx: ToSvg;
  sy: ToSvg;
  k: number;
}) {
  const { spec } = map;
  const range = pointsRange(points);
  const span = range ? range[1] - range[0] : 0;
  const unreached = new Set(map.unreached);
  const out = [];
  // The probed points, and the grid's still to come (refinement's aren't known yet).
  for (let i = 0; i < map.z.length; i++) {
    const p = points[i];
    if (p && range) {
      const t = span > 0 ? (p[2] - range[0]) / span : 0.5;
      out.push(<circle key={i} cx={sx(p[0])} cy={sy(p[1])} r={2.5 * k} style={{ fill: heatCss(t) }} />);
      continue;
    }
    if (!probed[i] && !onGrid(spec, i)) continue;
    const [x, y] = pointAt(spec, i);
    out.push(
      unreached.has(i) ? (
        <circle key={i} cx={sx(x)} cy={sy(y)} r={2.5 * k} fill="none" className="stroke-amber-500" strokeWidth={k} />
      ) : probed[i] ? (
        <circle key={i} cx={sx(x)} cy={sy(y)} r={2 * k} fill="none" className="stroke-muted-foreground" strokeWidth={0.75 * k} />
      ) : (
        <circle key={i} cx={sx(x)} cy={sy(y)} r={k} className="fill-muted-foreground/40" />
      ),
    );
  }
  return <g className="pointer-events-none">{out}</g>;
}

const OUTWARD: Record<Side, [number, number]> = { left: [-1, 0], right: [1, 0], front: [0, -1], back: [0, 1] };
const CORNER_SIDES: Record<Corner, [Side, Side]> = {
  frontLeft: ["left", "front"],
  frontRight: ["right", "front"],
  backLeft: ["left", "back"],
  backRight: ["right", "back"],
};

/** Where the selected routine will take the probe from where it is now (mirrors `hj_core::probe`). */
function ProbePreview({ form, at, ball, sx, sy, k }: { form: ProbeForm; at: [number, number]; ball: number; sx: ToSvg; sy: ToSvg; k: number }) {
  const r = ball / 2;
  const reachOut = (side: Side) => {
    const [dx, dy] = OUTWARD[side];
    const d = form.reach + r + 2;
    return <line key={side} x1={sx(at[0])} y1={sy(at[1])} x2={sx(at[0] + dx * d)} y2={sy(at[1] + dy * d)} />;
  };
  return (
    <g className="pointer-events-none fill-none stroke-sky-400/70" strokeWidth={k} strokeDasharray={`${4 * k} ${3 * k}`}>
      {form.kind === "edge" && (() => {
        // In from where it is, toward the part.
        const [dx, dy] = OUTWARD[form.side];
        const d = form.edgeSearch;
        return <line x1={sx(at[0])} y1={sy(at[1])} x2={sx(at[0] - dx * d)} y2={sy(at[1] - dy * d)} />;
      })()}
      {form.kind === "corner" && CORNER_SIDES[form.corner].map(reachOut)}
      {(form.kind === "bore" || form.kind === "boss") && form.across !== "xy" && (() => {
        // Out to either side along the one axis: as far as it searches, or steps out to.
        const d = form.kind === "bore" ? form.boreDiameter / 2 + 5 : form.bossDiameter / 2 + r + 3;
        const [dx, dy] = form.across === "x" ? [d, 0] : [0, d];
        return <line x1={sx(at[0] - dx)} y1={sy(at[1] - dy)} x2={sx(at[0] + dx)} y2={sy(at[1] + dy)} />;
      })()}
      {form.kind === "bore" && form.across === "xy" && <circle cx={sx(at[0])} cy={sy(at[1])} r={form.boreDiameter / 2} />}
      {form.kind === "boss" && form.across === "xy" && (
        <>
          <circle cx={sx(at[0])} cy={sy(at[1])} r={form.bossDiameter / 2} />
          <circle cx={sx(at[0])} cy={sy(at[1])} r={form.bossDiameter / 2 + r + 3} className="stroke-sky-400/40" />
        </>
      )}
    </g>
  );
}

/** A map point's coordinates beside it, in work coordinates (machine ones when not connected). */
function PointTip({
  label,
  pos: [x, y, z],
  offset,
  at: [left, top],
  width,
}: {
  label: string;
  pos: [number, number, number | null];
  offset: readonly number[] | null;
  at: [number, number];
  width: number;
}) {
  const o = offset ?? [0, 0, 0];
  // Right of the point, or left of it near the right edge.
  const flip = left > width - 190;
  return (
    <div
      className="pointer-events-none absolute z-10 rounded-md border bg-popover px-2 py-1 font-mono text-xs shadow-md"
      style={{ top: top - 12, ...(flip ? { right: width - left + 12 } : { left: left + 12 }) }}
    >
      <div className="font-sans text-[11px] text-muted-foreground">{label}</div>
      <div>X{(x - o[0]).toFixed(3)}</div>
      <div>Y{(y - o[1]).toFixed(3)}</div>
      {z !== null && <div>Z{(z - o[2]).toFixed(3)}</div>}
    </div>
  );
}

/**
 * Top-down view of the work area in machine coordinates: tool position, work zero and the program
 * preview, with Z in a bar beside it. Clicking a point offers to move the head there (X/Y only, at
 * the current height) or to put the work origin there without moving; the bar does the same for
 * Z. Or, switched to 3D, a view-only look at the program in space.
 */
export function BedView() {
  const connected = useConnected();
  const t = useStore((s) => s.telemetry);
  const status = useStatus();
  const state = useMachineState();
  const job = useJobRunning();
  const travel = useStore((s) => s.state?.machine?.travel);
  const speed = useUi((s) => s.jogSpeed);
  const svg = useRef<SVGSVGElement>(null);
  const [hover, setHover] = useState<Point | null>(null);
  const [hoverZ, setHoverZ] = useState<number | null>(null);
  const view = useUi((s) => s.mapView);
  const [target, setTarget] = useState<Point | null>(null);
  const analysis = useProgram((s) => s.analysis);
  const cursorLine = useProgram((s) => s.cursorLine);
  const following = useFollowing();
  const probe = useStore((s) => s.state?.probe ?? null);
  const probeForm = useUi((s) => s.probe);
  const surfaceMap = probe?.map ?? null;
  // On the Map tab, dragging picks the area to map (in machine coordinates while dragging).
  const picking =
    (probeForm.kind === "map" || (probeForm.kind === "contour" && !probeForm.contourInside)) && connected && !probe?.running;
  const [drag, setDrag] = useState<{ from: Point; to: Point; moved: boolean } | null>(null);
  const dragged = useRef(false);
  const progress = status?.job ?? null;

  const minX = travel?.min[0] ?? DEFAULT_MIN[0];
  const minY = travel?.min[1] ?? DEFAULT_MIN[1];
  const maxX = travel?.max[0] ?? -1;
  const maxY = travel?.max[1] ?? -1;
  const w = maxX - minX;
  const h = maxY - minY;

  // SVG y grows downwards; machine +Y is towards the back, drawn at the top.
  const sx = (x: number) => x - minX;
  const sy = (y: number) => maxY - y;

  // The drawing fills the panel. The view is its centre (drawing units) and a zoom, where 1 fits
  // the bed with its margin; the panel's size makes it a view box.
  const [zv, setZv] = useState<{ cx: number; cy: number; zoom: number } | null>(null);
  const fits = useUi((s) => s.fits);
  useEffect(() => setZv(null), [fits]);
  const area = useRef<HTMLDivElement>(null);
  const [panel, setPanel] = useState({ w: 0, h: 0 });
  useEffect(() => {
    const el = area.current;
    if (!el) return;
    const ro = new ResizeObserver(() => setPanel({ w: el.clientWidth, h: el.clientHeight }));
    ro.observe(el);
    return () => ro.disconnect();
  }, [view]);
  const fullW = w + 2 * MARGIN;
  const fullH = h + 2 * MARGIN;
  /** Screen pixels per drawing unit at zoom 1. */
  const fit = panel.w > 0 && panel.h > 0 ? Math.min(panel.w / fullW, panel.h / fullH) : 1;
  const view0 = { cx: w / 2, cy: h / 2, zoom: 1 };
  /** As in an image viewer: along each axis, the view pans only as far as the bed (with its
   * margin) runs past the panel, and stays centred where it doesn't. Also after a resize. */
  const clampView = (cx: number, cy: number, z: number, at = { fit, panel }) => {
    const zoom = Math.min(Math.max(z, 1), MAX_ZOOM);
    const ppu = at.fit * zoom;
    const axis = (c: number, size: number, shown: number) =>
      shown >= size + 2 * MARGIN ? size / 2 : Math.min(Math.max(c, -MARGIN + shown / 2), size + MARGIN - shown / 2);
    return { cx: axis(cx, w, at.panel.w / ppu), cy: axis(cy, h, at.panel.h / ppu), zoom };
  };
  const cur = zv ? clampView(zv.cx, zv.cy, zv.zoom) : view0;
  const zoom = cur.zoom;
  const ppu = fit * zoom;
  const k = 1 / ppu;
  const vb = { x: cur.cx - panel.w / ppu / 2, y: cur.cy - panel.h / ppu / 2, w: panel.w / ppu, h: panel.h / ppu };
  // A native listener: React's wheel handler is passive, and a pinch would zoom the whole page.
  const viewInputs = useRef({ fit, panel, view0 });
  viewInputs.current = { fit, panel, view0 };
  useEffect(() => {
    const el = svg.current;
    if (!el) return;
    const onWheel = (e: WheelEvent) => {
      e.preventDefault();
      const box = el.getBoundingClientRect();
      // Pinches come as small wheel deltas with Ctrl held.
      const factor = Math.exp(-e.deltaY * (e.ctrlKey ? 0.01 : 0.002));
      const dx = e.clientX - box.left - box.width / 2;
      const dy = e.clientY - box.top - box.height / 2;
      // From the latest view (several wheel events can come before a render), keeping the point
      // under the cursor where it is.
      setZv((prev) => {
        const { fit, panel, view0 } = viewInputs.current;
        const p = prev ? clampView(prev.cx, prev.cy, prev.zoom, { fit, panel }) : view0;
        const next = Math.min(Math.max(p.zoom * factor, 1), MAX_ZOOM);
        const [before, after] = [fit * p.zoom, fit * next];
        return clampView(p.cx + dx / before - dx / after, p.cy + dy / before - dy / after, next, { fit, panel });
      });
    };
    el.addEventListener("wheel", onWheel, { passive: false });
    return () => el.removeEventListener("wheel", onWheel);
    // Everything else clampView reads comes from viewInputs.
  }, [view, w, h]);
  // Middle-drag pans (as in 3D), followed outside the drawing until released.
  const startPan = (e: React.MouseEvent) => {
    if (e.button !== 1) return;
    e.preventDefault();
    const from = { x: e.clientX, y: e.clientY, cx: cur.cx, cy: cur.cy };
    const move = (m: MouseEvent) =>
      setZv((prev) =>
        clampView(from.cx - (m.clientX - from.x) * k, from.cy - (m.clientY - from.y) * k, (prev ?? view0).zoom, viewInputs.current),
      );
    const up = () => {
      window.removeEventListener("mousemove", move);
      window.removeEventListener("mouseup", up);
    };
    window.addEventListener("mousemove", move);
    window.addEventListener("mouseup", up);
  };

  const canAct = connected && state === "idle" && !job;
  /** The clicked point, and where to anchor the menu (relative to the drawing's container). */
  const [menu, setMenu] = useState<{ p: Point; left: number; top: number } | null>(null);

  const openMenu = (e: React.MouseEvent) => {
    if (dragged.current) {
      dragged.current = false;
      return;
    }
    if (hoverOutline !== null) {
      e.preventDefault();
      selectOutline(hoverOutline);
      return;
    }
    if (hoverPoint !== null && connected) {
      const box = area.current?.getBoundingClientRect();
      if (!box) return;
      e.preventDefault();
      setPointMenu({ target: hoverPoint, left: e.clientX - box.left, top: e.clientY - box.top });
      return;
    }
    const p = toMachine(e);
    const box = area.current?.getBoundingClientRect();
    if (!p || !box || !connected) return;
    e.preventDefault();
    // To the 0.01 mm the menu shows, so the origin lands on the displayed coordinates.
    const round = (v: number) => Math.round(v * 100) / 100;
    setMenu({ p: { x: round(p.x), y: round(p.y) }, left: e.clientX - box.left, top: e.clientY - box.top });
  };
  const moveHere = (p: Point) => {
    setTarget(p);
    void act({ type: "moveTo", x: p.x, y: p.y, speed }).then((ok) => ok || setTarget(null));
  };

  // Clear the target marker once the move is over: back to idle after running (the target may
  // have been clamped to the travel limits, so it isn't always reached), or stopped otherwise.
  const sawRun = useRef(false);
  useEffect(() => {
    if (!target) {
      sawRun.current = false;
      return;
    }
    if (state === "run") sawRun.current = true;
    if ((state === "idle" && sawRun.current) || !canJog(state, job)) setTarget(null);
  }, [target, state, job]);

  const toMachine = (e: { clientX: number; clientY: number }, clamp = false): Point | null => {
    const el = svg.current;
    const ctm = el?.getScreenCTM();
    if (!el || !ctm) return null;
    const p = new DOMPoint(e.clientX, e.clientY).matrixTransform(ctm.inverse());
    let x = p.x + minX;
    let y = maxY - p.y;
    if (clamp) {
      x = Math.min(Math.max(x, minX), maxX);
      y = Math.min(Math.max(y, minY), maxY);
    } else if (x < minX || x > maxX || y < minY || y > maxY) return null;
    return { x, y };
  };

  // Zooming and panning move the drawing under a still pointer: keep the crosshair on what's under it.
  const pointer = useRef<{ clientX: number; clientY: number } | null>(null);
  useEffect(() => {
    if (pointer.current) setHover(toMachine(pointer.current));
    // toMachine reads the view through the SVG's transform, which this render just updated.
  }, [vb.x, vb.y, vb.w, vb.h]);

  const startDrag = (e: React.MouseEvent) => {
    if (!picking || e.button !== 0) return;
    const p = toMachine(e);
    if (p) setDrag({ from: p, to: p, moved: false });
  };
  const moveDrag = (e: React.MouseEvent) => {
    if (!drag) return;
    const to = toMachine(e, true)!;
    setDrag({ ...drag, to, moved: drag.moved || Math.hypot(to.x - drag.from.x, to.y - drag.from.y) > 1 });
  };
  const endDrag = () => {
    if (!drag) return;
    if (drag.moved && offset) {
      // To 0.1 mm in work coordinates (round numbers in the fields), kept in machine ones.
      const w = (v: number, i: number) => Math.round((v - offset[i]) * 10) / 10;
      const [ax, bx] = [w(drag.from.x, 0), w(drag.to.x, 0)].sort((a, b) => a - b);
      const [ay, by] = [w(drag.from.y, 1), w(drag.to.y, 1)].sort((a, b) => a - b);
      if (bx > ax && by > ay) {
        const [min, max] = areaOf([ax, ay], [bx, by], offset);
        setProbeForm(probeForm.kind === "contour" ? { contourMin: min, contourMax: max } : { mapMin: min, mapMax: max });
      }
      dragged.current = true;
    }
    setDrag(null);
  };

  // Lines every 10 mm, 1 mm or 0.1 mm, the finest that are far enough apart to see, and stronger
  // every 10 and 50 of them; only those in view, and only over the bed.
  const grid = [];
  const step = [10, 1, 0.1].filter((d) => d * ppu >= GRID_MIN_PX).at(-1) ?? 10;
  const line = (i: number) => {
    const mm = Math.round(i * step * 10);
    return mm % 500 === 0 ? ["stroke-border", 1] : mm % 100 === 0 ? ["stroke-border/60", 0.75] : mm % 10 === 0 && step < 1 ? ["stroke-border/50", 0.75] : ["stroke-border/25", 0.75];
  };
  for (const [axis, size, from, to] of [
    ["x", w, vb.x, vb.x + vb.w],
    ["y", h, vb.y, vb.y + vb.h],
  ] as const) {
    for (let i = Math.max(0, Math.ceil(from / step)); i * step <= Math.min(size, to); i++) {
      const v = i * step;
      const [cls, width] = line(i) as [string, number];
      grid.push(
        axis === "x" ? (
          <line key={`x${i}`} x1={v} y1={0} x2={v} y2={h} className={cls} strokeWidth={width * k} />
        ) : (
          <line key={`y${i}`} x1={0} y1={v} x2={w} y2={v} className={cls} strokeWidth={width * k} />
        ),
      );
    }
  }

  const tool = connected && t ? { x: sx(t.machine[0]), y: sy(t.machine[1]) } : null;
  const zero = connected && status ? { x: sx(status.workOffset[0]), y: sy(status.workOffset[1]) } : null;
  const offset = status?.workOffset;

  // The program in the editor, placed at the current work zero.
  const ox = offset?.[0];
  const oy = offset?.[1];
  const doneLine = following && progress ? progress.lines : 0;
  const preview = useMemo(() => {
    if (!analysis || ox === undefined || oy === undefined) return null;
    const map = (x: number, y: number): [number, number] => [x + ox - minX, maxY - (y + oy)];
    const n = analysis.kinds.length;
    const split = doneLine > 0 ? vertexAfterLine(analysis, doneLine) : 0;
    const done = pathData(analysis, 0, split, map);
    const rest = pathData(analysis, split, n, map);
    const b = analysis.cutBounds;
    const box = b && { x: b.min[0] + ox - minX, y: maxY - (b.max[1] + oy), w: b.max[0] - b.min[0], h: b.max[1] - b.min[1] };
    return { map, done, rest, box };
  }, [analysis, ox, oy, minX, maxY, doneLine]);
  // The program as a transform in the sidebar would leave it, while that's being set up.
  const transforms = useUi((s) => s.transforms);
  const ghost = useMemo(() => {
    const f = previewOf(transforms);
    if (!f || !transforms || !analysis || ox === undefined || oy === undefined) return null;
    const map = (x: number, y: number): [number, number] => {
      const [tx, ty] = f(x, y, 0);
      return [tx + ox - minX, maxY - (ty + oy)];
    };
    const p = pathData(analysis, 0, analysis.kinds.length, map);
    const pivot = transforms.preview === "rotate" ? map(...transforms.rotate.pivot) : null;
    return { feed: p.feed, rapid: p.rapid, pivot };
  }, [transforms, analysis, ox, oy, minX, maxY]);
  const highlight = useMemo(() => {
    if (!preview || !analysis || cursorLine === null) return null;
    const from = vertexAfterLine(analysis, cursorLine - 1);
    const to = vertexAfterLine(analysis, cursorLine);
    if (from === to) return null;
    const p = pathData(analysis, from, to, preview.map);
    return p.rapid + p.feed;
  }, [preview, analysis, cursorLine]);
  const plan = picking && probeForm.kind === "map" ? plannedSpec(probeForm) : null;
  const contourBox = picking && probeForm.kind === "contour" ? plannedContour(probeForm) : null;
  // Drawn again only when the map changes, not with every position report: a map can be thousands
  // of points.
  const compensate = useUi((s) => s.mapCompensate);
  const ball = probe?.settings.ballDiameter ?? 0;
  const mapPoints = useMemo(() => (surfaceMap ? surfacePoints(surfaceMap, ball / 2, compensate) : []), [surfaceMap, ball, compensate]);
  const mapProbed = useMemo(() => (surfaceMap ? probedMask(surfaceMap) : []), [surfaceMap]);
  const measured = useMemo(
    () =>
      surfaceMap && <MeasuredMap map={surfaceMap} probed={mapProbed} points={mapPoints} sx={(x) => x - minX} sy={(y) => maxY - y} k={k} />,
    [surfaceMap, mapProbed, mapPoints, minX, maxY, k],
  );
  const contour = probe?.contour ?? null;
  const contourWall = useMemo(
    () => (contour ? wallPoints(contour.points as V2[], ball / 2, compensate, contour.closed) : []),
    [contour, ball, compensate],
  );
  // Markers, where they are on the bed (null for a work one without a work offset). The offset is
  // rounded so they aren't placed again with every position report.
  const markers = useMarkers((s) => s.markers);
  const listHovered = useMarkers((s) => s.hovered);
  const mOffset = useWorkOffset();
  const [mo0, mo1, mo2] = (mOffset ?? []).map((v) => Math.round(v * 1e4) / 1e4);
  const markerAt = useMemo(
    () => markers.map((m) => machinePos(m, mo0 === undefined ? null : [mo0, mo1, mo2])),
    [markers, mo0, mo1, mo2],
  );
  // The marker, map or contour point under the pointer (within a few pixels): highlighted, with its
  // coordinates, and a click on it opens its own menu.
  const hoverPoint = useMemo((): Target | null => {
    if (!hover || drag) return null;
    const radius = 8 * k;
    let best: Target | null = null;
    let bestD = radius;
    markerAt.forEach((p, i) => {
      const d = p ? Math.hypot(p[0] - hover.x, p[1] - hover.y) : Infinity;
      if (d <= bestD) [best, bestD] = [{ kind: "marker", i }, d];
    });
    if (best) return best;
    if (mo0 !== undefined && Math.hypot(mo0 - hover.x, mo1 - hover.y) <= radius) return { kind: "origin", i: 0 };
    contourWall.forEach(([x, y], i) => {
      const d = Math.hypot(x - hover.x, y - hover.y);
      if (d <= bestD) [best, bestD] = [{ kind: "contour", i }, d];
    });
    if (best || !surfaceMap) return best;
    const i = nearestPoint(surfaceMap, mapProbed, mapPoints, hover.x, hover.y, radius);
    return i === null ? null : { kind: "map", i };
  }, [surfaceMap, mapProbed, mapPoints, contourWall, markerAt, mo0, mo1, hover, drag, k]);
  /** A point's machine position. A map's: the surface point, or the grid point where there's none.
   * A contour's: on the wall (or where the ball's centre was), and no Z: it's a wall. */
  const pointPos = (p: Target): [number, number, number | null] => {
    if (p.kind === "marker") return markerAt[p.i]!;
    // Where work X/Y/Z 0 is, Z for the tool that's in.
    if (p.kind === "origin") return [mo0, mo1, mo2];
    if (p.kind === "contour") return [...contourWall[p.i], null];
    const q = mapPoints[p.i];
    if (q) return q;
    const [x, y] = pointAt(surfaceMap!.spec, p.i);
    return [x, y, null];
  };
  const workOffset = useWorkOffset();
  /** "Point 3, 5" (grid column, row from the front left), and what became of it if there's no
   * surface; or "Contour point 12". */
  const pointLabel = (p: Target) => {
    if (p.kind === "marker") return markerLabel(markers[p.i], p.i);
    if (p.kind === "origin") return "Work origin";
    if (p.kind === "contour") return `Contour point ${p.i + 1}`;
    const i = p.i;
    const m = surfaceMap!;
    const what = mapPoints[i] ? "" : m.unreached.includes(i) ? " · out of the stylus' reach" : " · no contact";
    // In grid units: refined points fall between grid points.
    const [cols] = lattice(m.spec);
    const s = scaleOf(m.spec);
    const g = (v: number) => String(+(v / s + 1).toFixed(3));
    return `Point ${g(i % cols)}, ${g(Math.floor(i / cols))}${what}`;
  };
  const [pointMenu, setPointMenu] = useState<{ target: Target; left: number; top: number } | null>(null);
  // A reference drawing: its outlines on the bed (by the fit, or at the work origin before), and,
  // while one is being picked, the one under the pointer.
  const drawing = useDrawing((s) => s.current);
  const ox2 = offset?.[0];
  const oy2 = offset?.[1];
  const placed = useMemo(() => {
    const place = drawing && placement(drawing, ox2 === undefined || oy2 === undefined ? null : [ox2, oy2]);
    return drawing && place ? drawing.drawing.outlines.map((o) => (o.points as V2[]).map(place)) : null;
  }, [drawing, ox2, oy2]);
  const pickingOutline = drawing !== null && drawing.fits === null;
  const hoverOutline = useMemo(() => {
    if (!pickingOutline || !placed || !hover || drag) return null;
    let best: number | null = null;
    let bestD = 6 * k;
    placed.forEach((pts, i) => {
      if (!drawing!.drawing.outlines[i].closed) return;
      const d = distanceTo(pts, true, [hover.x, hover.y]);
      if (d <= bestD) [best, bestD] = [i, d];
    });
    return best;
  }, [pickingOutline, placed, hover, drag, k, drawing]);
  const problems =
    analysis && offset && travel ? fitProblems(analysis, offset, travel) : [];

  return (
    <div className="flex h-full flex-col">
      <div className="flex h-11 shrink-0 items-center gap-3 border-b px-3">
        <div className="font-mono text-xs">
          {view === "2d" && hoverZ !== null && (
            <>
              {t && connected && <span>W Z{(hoverZ - (t.machine[2] - t.work[2])).toFixed(2)}</span>}
              <span className="ml-3 text-muted-foreground">M Z{hoverZ.toFixed(2)}</span>
            </>
          )}
          {view === "2d" && hover && (
            <>
              {offset && (
                <span>
                  W X{(hover.x - offset[0]).toFixed(2)} Y{(hover.y - offset[1]).toFixed(2)}
                </span>
              )}
              <span className="ml-3 text-muted-foreground">
                M X{hover.x.toFixed(2)} Y{hover.y.toFixed(2)}
              </span>
            </>
          )}
        </div>
        <div className="flex-1" />
        {problems.length > 0 && (
          <span
            className="flex min-w-0 items-center gap-1 text-xs text-amber-500"
            title={`${problems.map((p) => p.detail).join("\n")}\n\nWith the current work zero, the program goes past the machine's soft limits.`}
          >
            <TriangleAlert className="size-3.5 shrink-0" />
            <span className="truncate">Outside travel: {problems.map((p) => p.short).join(", ")}</span>
          </span>
        )}
        <div className="flex shrink-0 rounded-md border p-0.5 text-xs" role="radiogroup" aria-label="Map view">
          {(["2d", "3d"] as const).map((v) => (
            <button
              key={v}
              type="button"
              role="radio"
              aria-checked={view === v}
              className={cn(
                "rounded-sm px-2 py-0.5 font-medium uppercase",
                view === v ? "bg-muted text-foreground" : "text-muted-foreground hover:text-foreground",
              )}
              onClick={() => setMapView(v)}
            >
              {v}
            </button>
          ))}
        </div>
      </div>

      {view === "3d" ? (
        <Suspense fallback={<div className="flex-1" />}>
          <Bed3D />
        </Suspense>
      ) : (
        <div className="flex min-h-0 flex-1 gap-2 overflow-hidden p-2 pr-1">
          <div ref={area} className="relative min-w-0 flex-1">
            {!connected && (
              <div className="pointer-events-none absolute top-2 left-1/2 z-10 w-max max-w-[80%] -translate-x-1/2 rounded-md bg-background/80 px-3 py-1.5 text-center text-xs text-muted-foreground ring-1 ring-foreground/10">
                Not connected
              </div>
            )}
            {zoom > 1 && (
              <Button
                size="xs"
                variant="outline"
                className="absolute top-1 left-1 z-10 bg-background/70"
                title="Show the whole bed (scroll to zoom, middle-drag to pan)"
                onClick={() => setZv(null)}
              >
                <Focus /> Fit · {zoom < 10 ? zoom.toFixed(1) : Math.round(zoom)}×
              </Button>
            )}
            <DropdownMenu open={menu !== null} onOpenChange={(open) => !open && setMenu(null)}>
              <DropdownMenuTrigger asChild>
                <span className="pointer-events-none absolute size-0" style={{ left: menu?.left ?? 0, top: menu?.top ?? 0 }} />
              </DropdownMenuTrigger>
              {menu && (
                <DropdownMenuContent align="start" sideOffset={6} className="min-w-56">
                  <DropdownMenuLabel className="font-mono text-xs font-normal">
                    {offset && (
                      <div>
                        W X{(menu.p.x - offset[0]).toFixed(2)} Y{(menu.p.y - offset[1]).toFixed(2)}
                      </div>
                    )}
                    <div className="text-muted-foreground">
                      M X{menu.p.x.toFixed(2)} Y{menu.p.y.toFixed(2)}
                    </div>
                  </DropdownMenuLabel>
                  <DropdownMenuSeparator />
                  <DropdownMenuItem disabled={!canAct} onSelect={() => moveHere(menu.p)}>
                    <Move /> Move head here
                    <DropdownMenuShortcut>{Math.round(speed * 100)}%</DropdownMenuShortcut>
                  </DropdownMenuItem>
                  <DropdownMenuItem
                    disabled={!canAct}
                    onSelect={() => void act({ type: "setOrigin", x: menu.p.x, y: menu.p.y })}
                  >
                    <Crosshair /> Set work origin here
                    <DropdownMenuShortcut>X/Y</DropdownMenuShortcut>
                  </DropdownMenuItem>
                  <DropdownMenuSeparator />
                  <DropdownMenuItem onSelect={() => addMarker([menu.p.x, menu.p.y, null], workOffset)}>
                    <MapPin /> Add marker here
                  </DropdownMenuItem>
                </DropdownMenuContent>
              )}
            </DropdownMenu>
            {hoverPoint !== null && !pointMenu && (
              <PointTip
                label={pointLabel(hoverPoint)}
                pos={pointPos(hoverPoint)}
                offset={workOffset}
                at={[(sx(pointPos(hoverPoint)[0]) - vb.x) * ppu, (sy(pointPos(hoverPoint)[1]) - vb.y) * ppu]}
                width={panel.w}
              />
            )}
            <DropdownMenu open={pointMenu !== null} onOpenChange={(open) => !open && setPointMenu(null)}>
              <DropdownMenuTrigger asChild>
                <span className="pointer-events-none absolute size-0" style={{ left: pointMenu?.left ?? 0, top: pointMenu?.top ?? 0 }} />
              </DropdownMenuTrigger>
              {pointMenu && (() => {
                const pos = pointPos(pointMenu.target);
                const { kind, i } = pointMenu.target;
                return (
                  <DropdownMenuContent align="start" sideOffset={6} className="min-w-60">
                    <DropdownMenuLabel className="font-mono text-xs font-normal">
                      <div className="font-sans font-medium">{pointLabel(pointMenu.target)}</div>
                      {workOffset && <div>W {coordsText(pos, workOffset)}</div>}
                      <div className="text-muted-foreground">M {coordsText(pos, [0, 0, 0])}</div>
                    </DropdownMenuLabel>
                    <DropdownMenuSeparator />
                    <PointActions
                      pos={pos}
                      workOffset={workOffset}
                      canAct={canAct}
                      speed={speed}
                      onMove={(x, y) => moveHere({ x, y })}
                      isOrigin={kind === "origin"}
                    />
                    <DropdownMenuSeparator />
                    {kind === "marker" ? (
                      <DropdownMenuItem variant="destructive" onSelect={() => removeMarker(markers[i].id)}>
                        <Trash2 /> Delete marker
                      </DropdownMenuItem>
                    ) : (
                      <DropdownMenuItem onSelect={() => addMarker(pos, workOffset)}>
                        <MapPin /> Add marker here
                      </DropdownMenuItem>
                    )}
                  </DropdownMenuContent>
                );
              })()}
            </DropdownMenu>
            <svg
              ref={svg}
              viewBox={`${vb.x} ${vb.y} ${Math.max(vb.w, 1e-6)} ${Math.max(vb.h, 1e-6)}`}
              className={cn("absolute inset-0 block size-full", connected && (hoverPoint !== null || hoverOutline !== null ? "cursor-pointer" : "cursor-crosshair"))}
              onMouseMove={(e) => {
                pointer.current = { clientX: e.clientX, clientY: e.clientY };
                setHover(toMachine(e));
                moveDrag(e);
              }}
              onMouseLeave={() => {
                pointer.current = null;
                setHover(null);
                endDrag();
              }}
              onMouseDown={(e) => {
                startPan(e);
                startDrag(e);
              }}
              onMouseUp={endDrag}
              onClick={openMenu}
              onContextMenu={openMenu}
            >
              <rect x={0} y={0} width={w} height={h} className="fill-muted/30 stroke-border" strokeWidth={1.5 * k} />
              {grid}
              {preview && (
                <g className="pointer-events-none" fill="none" strokeLinejoin="round" strokeLinecap="round">
                  {preview.box && (
                    <rect
                      {...{ x: preview.box.x, y: preview.box.y, width: preview.box.w, height: preview.box.h }}
                      className="stroke-fuchsia-400/30"
                      strokeWidth={0.75 * k}
                      strokeDasharray={`${4 * k} ${3 * k}`}
                    />
                  )}
                  <path d={preview.done.rapid} className="stroke-muted-foreground/20" strokeWidth={0.75 * k} strokeDasharray={`${3 * k} ${3 * k}`} />
                  <path d={preview.done.feed} className="stroke-fuchsia-400/25" strokeWidth={1.25 * k} />
                  <path d={preview.rest.rapid} className="stroke-muted-foreground/50" strokeWidth={0.75 * k} strokeDasharray={`${3 * k} ${3 * k}`} />
                  <path d={preview.rest.feed} className="stroke-fuchsia-400" strokeWidth={1.25 * k} />
                  {highlight && <path d={highlight} className="stroke-yellow-300" strokeWidth={2.5 * k} />}
                </g>
              )}
              {ghost && (
                <g className="pointer-events-none" fill="none" strokeLinejoin="round" strokeLinecap="round">
                  <path d={ghost.rapid} className="stroke-amber-300/40" strokeWidth={0.75 * k} strokeDasharray={`${3 * k} ${3 * k}`} />
                  <path d={ghost.feed} className="stroke-amber-300" strokeWidth={1.25 * k} strokeDasharray={`${5 * k} ${3 * k}`} />
                  {ghost.pivot && <circle cx={ghost.pivot[0]} cy={ghost.pivot[1]} r={4 * k} className="stroke-amber-300" strokeWidth={1.5 * k} />}
                </g>
              )}
              {probe && <ProbeOverlay probe={probe} sx={sx} sy={sy} h={h} w={w} k={k} />}
            {measured}
            {hoverPoint !== null && (() => {
              const [x, y] = pointPos(hoverPoint);
              return <circle cx={sx(x)} cy={sy(y)} r={6 * k} fill="none" className="pointer-events-none stroke-sky-300" strokeWidth={1.5 * k} />;
            })()}
            {plan && !drag?.moved && <PlannedMap spec={plan} dots={!surfaceMap?.probed.length} sx={sx} sy={sy} k={k} />}
            {contourBox && !drag?.moved && (
              <rect
                x={sx(contourBox.min[0])}
                y={sy(contourBox.max[1])}
                width={contourBox.max[0] - contourBox.min[0]}
                height={contourBox.max[1] - contourBox.min[1]}
                className="pointer-events-none fill-sky-400/5 stroke-sky-400/70"
                strokeWidth={k}
                strokeDasharray={`${4 * k} ${3 * k}`}
              />
            )}
            {drawing && placed && (
              <g className="pointer-events-none" fill="none" strokeLinejoin="round">
                {placed.map((pts, i) => {
                  const o = drawing.drawing.outlines[i];
                  const d = pts.map(([x, y], j) => `${j ? "L" : "M"}${sx(x)} ${sy(y)}`).join("") + (o.closed ? "Z" : "");
                  const picked = i === drawing.selected;
                  const cls = picked
                    ? "stroke-violet-300"
                    : i === hoverOutline
                      ? "stroke-violet-300/80"
                      : o.closed
                        ? "stroke-violet-400/50"
                        : "stroke-violet-400/30";
                  return (
                    <path
                      key={i}
                      d={d}
                      className={cls}
                      strokeWidth={(picked || i === hoverOutline ? 2 : 1) * k}
                      strokeDasharray={o.closed ? undefined : `${3 * k} ${3 * k}`}
                    />
                  );
                })}
              </g>
            )}
            {contour && contourWall.length > 0 && (
              <g className="pointer-events-none">
                {contourWall.length > 1 &&
                  (() => {
                    const d = contourWall.map(([x, y], i) => `${i ? "L" : "M"}${sx(x)} ${sy(y)}`).join("") + (contour.closed ? "Z" : "");
                    return <path d={d} fill="none" className="stroke-emerald-400" strokeWidth={1.5 * k} strokeLinejoin="round" />;
                  })()}
                {contourWall.map(([x, y], i) => (
                  <circle key={i} cx={sx(x)} cy={sy(y)} r={2 * k} className="fill-emerald-300" />
                ))}
              </g>
            )}
            {markerAt.map((p, i) => {
              if (!p) return null;
              const [x, y] = [sx(p[0]), sy(p[1])];
              const strong = markers[i].id === listHovered;
              const r = (strong ? 6 : 4.5) * k;
              const name = markers[i].name.trim();
              return (
                <g key={markers[i].id} className="pointer-events-none">
                  <path
                    d={`M${x} ${y - r}L${x + r} ${y}L${x} ${y + r}L${x - r} ${y}Z`}
                    className={strong ? "fill-amber-300 stroke-background" : "fill-amber-400 stroke-background"}
                    strokeWidth={k}
                  />
                  {name && (
                    <text
                      x={x + 8 * k}
                      y={y}
                      dominantBaseline="central"
                      fontSize={11 * k}
                      className={cn("fill-amber-200 stroke-background", strong && "fill-amber-100")}
                      strokeWidth={3 * k}
                      paintOrder="stroke"
                      strokeLinejoin="round"
                    >
                      {name}
                    </text>
                  )}
                </g>
              );
            })}
            {drag?.moved && (
              <rect
                x={sx(Math.min(drag.from.x, drag.to.x))}
                y={sy(Math.max(drag.from.y, drag.to.y))}
                width={Math.abs(drag.to.x - drag.from.x)}
                height={Math.abs(drag.to.y - drag.from.y)}
                className="pointer-events-none fill-sky-400/10 stroke-sky-400"
                strokeWidth={k}
              />
            )}
              {tool && t && probe && !probe.running && isProbe(status?.tool?.number) && state === "idle" && (
                <ProbePreview form={probeForm} at={[t.machine[0], t.machine[1]]} ball={probe.settings.ballDiameter} sx={sx} sy={sy} k={k} />
              )}
              {zero && (
                <g className="stroke-emerald-500" strokeWidth={1.5 * k}>
                  <line x1={zero.x - 15 * k} y1={zero.y} x2={zero.x + 15 * k} y2={zero.y} />
                  <line x1={zero.x} y1={zero.y - 15 * k} x2={zero.x} y2={zero.y + 15 * k} />
                  <circle cx={zero.x} cy={zero.y} r={6 * k} fill="none" />
                </g>
              )}
              {hover && connected && !menu && hoverPoint === null && !pointMenu && (
                <g className="pointer-events-none stroke-sky-400/50" strokeWidth={0.75 * k}>
                  <line x1={sx(hover.x)} y1={0} x2={sx(hover.x)} y2={h} />
                  <line x1={0} y1={sy(hover.y)} x2={w} y2={sy(hover.y)} />
                </g>
              )}
              {menu && (
                <circle
                  cx={sx(menu.p.x)}
                  cy={sy(menu.p.y)}
                  r={5 * k}
                  fill="none"
                  className="pointer-events-none stroke-sky-400"
                  strokeWidth={1.5 * k}
                />
              )}
              {target && (
                <g className="stroke-amber-400" strokeWidth={1.5 * k}>
                  {tool && <line x1={tool.x} y1={tool.y} x2={sx(target.x)} y2={sy(target.y)} strokeDasharray={`${5 * k} ${4 * k}`} />}
                  <circle cx={sx(target.x)} cy={sy(target.y)} r={5 * k} fill="none" />
                </g>
              )}
              {tool && (
                <g>
                  <circle cx={tool.x} cy={tool.y} r={8 * k} className="fill-sky-500/30 stroke-sky-400" strokeWidth={1.5 * k} />
                  <circle cx={tool.x} cy={tool.y} r={2 * k} className="fill-sky-400" />
                </g>
              )}
              <text x={w / 2} y={h + 14 * k} textAnchor="middle" className="fill-muted-foreground" style={{ fontSize: 11 * k }}>
                front
              </text>
            </svg>
          </div>
          <ZBar height={panel.h} pad={Z_BAR_PAD} onHover={setHoverZ} />
        </div>
      )}
    </div>
  );
}
