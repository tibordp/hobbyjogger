import { Crosshair, Move, TriangleAlert } from "lucide-react";
import { useEffect, useMemo, useRef, useState } from "react";

import type { Analysis } from "@/bindings/Analysis";
import type { Corner } from "@/bindings/Corner";
import type { ProbeState } from "@/bindings/ProbeState";
import type { Side } from "@/bindings/Side";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuLabel,
  DropdownMenuSeparator,
  DropdownMenuShortcut,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import { act } from "@/lib/actions";
import { canJog, isProbe, useConnected, useJobRunning, useMachineState, useStatus } from "@/lib/machine";
import { fitProblems, useFollowing, useProgram } from "@/lib/program";
import { useStore } from "@/lib/store";
import { useUi, type ProbeForm } from "@/lib/ui";
import { cn } from "@/lib/utils";

/** Used until the machine reports its limits (Z1 values). */
const DEFAULT_MIN = [-207, -206];
const MARGIN = 8;

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

type ToSvg = (v: number) => number;

/** Contacts of the current or last probing routine, and what it found. */
function ProbeOverlay({ probe, sx, sy, w, h }: { probe: ProbeState; sx: ToSvg; sy: ToSvg; w: number; h: number }) {
  const r = probe.result;
  return (
    <g className="pointer-events-none">
      {probe.contacts.map((c, i) => (
        <circle key={i} cx={sx(c[0])} cy={sy(c[1])} r={0.9} className="fill-sky-300" />
      ))}
      {r && !probe.running && (
        <g className="fill-none stroke-sky-300" strokeWidth={0.5}>
          {r.x !== null && r.y === null && <line x1={sx(r.x)} y1={0} x2={sx(r.x)} y2={h} strokeDasharray="2 1.5" />}
          {r.y !== null && r.x === null && <line x1={0} y1={sy(r.y)} x2={w} y2={sy(r.y)} strokeDasharray="2 1.5" />}
          {r.x !== null && r.y !== null && (
            <>
              <line x1={sx(r.x) - 5} y1={sy(r.y)} x2={sx(r.x) + 5} y2={sy(r.y)} />
              <line x1={sx(r.x)} y1={sy(r.y) - 5} x2={sx(r.x)} y2={sy(r.y) + 5} />
              {r.diameter !== null && <circle cx={sx(r.x)} cy={sy(r.y)} r={r.diameter / 2} />}
            </>
          )}
        </g>
      )}
    </g>
  );
}

const OUTWARD: Record<Side, [number, number]> = { left: [-1, 0], right: [1, 0], front: [0, -1], back: [0, 1] };
const CORNER_SIDES: Record<Corner, [Side, Side]> = {
  frontLeft: ["left", "front"],
  frontRight: ["right", "front"],
  backLeft: ["left", "back"],
  backRight: ["right", "back"],
};

/** Where the selected routine will take the probe from where it is now (mirrors `hj_core::probe`). */
function ProbePreview({ form, at, ball, sx, sy }: { form: ProbeForm; at: [number, number]; ball: number; sx: ToSvg; sy: ToSvg }) {
  const r = ball / 2;
  const reachOut = (side: Side) => {
    const [dx, dy] = OUTWARD[side];
    const d = form.reach + r + 2;
    return <line key={side} x1={sx(at[0])} y1={sy(at[1])} x2={sx(at[0] + dx * d)} y2={sy(at[1] + dy * d)} />;
  };
  return (
    <g className="pointer-events-none fill-none stroke-sky-400/70" strokeWidth={0.4} strokeDasharray="1.5 1">
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

/**
 * Top-down view of the work area in machine coordinates: tool position, work zero and the program
 * preview. Clicking a point offers to move the head there (X/Y only, at the current height) or to
 * put the work origin there without moving. Placeholder for the 3D view.
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
  const [target, setTarget] = useState<Point | null>(null);
  const analysis = useProgram((s) => s.analysis);
  const cursorLine = useProgram((s) => s.cursorLine);
  const following = useFollowing();
  const probe = useStore((s) => s.state?.probe ?? null);
  const probeForm = useUi((s) => s.probe);
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

  const canAct = connected && state === "idle" && !job;
  /** The clicked point, and where to anchor the menu (relative to the drawing's container). */
  const [menu, setMenu] = useState<{ p: Point; left: number; top: number } | null>(null);
  const area = useRef<HTMLDivElement>(null);

  const openMenu = (e: React.MouseEvent) => {
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

  const toMachine = (e: React.MouseEvent): Point | null => {
    const el = svg.current;
    const ctm = el?.getScreenCTM();
    if (!el || !ctm) return null;
    const p = new DOMPoint(e.clientX, e.clientY).matrixTransform(ctm.inverse());
    const x = p.x + minX;
    const y = maxY - p.y;
    if (x < minX || x > maxX || y < minY || y > maxY) return null;
    return { x, y };
  };

  const grid = [];
  for (let v = 0; v <= w; v += 10) {
    const major = Math.round(v) % 50 === 0;
    grid.push(
      <line key={`x${v}`} x1={v} y1={0} x2={v} y2={h} className={major ? "stroke-border" : "stroke-border/40"} strokeWidth={major ? 0.4 : 0.2} />,
    );
  }
  for (let v = 0; v <= h; v += 10) {
    const major = Math.round(v) % 50 === 0;
    grid.push(
      <line key={`y${v}`} x1={0} y1={v} x2={w} y2={v} className={major ? "stroke-border" : "stroke-border/40"} strokeWidth={major ? 0.4 : 0.2} />,
    );
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
  const highlight = useMemo(() => {
    if (!preview || !analysis || cursorLine === null) return null;
    const from = vertexAfterLine(analysis, cursorLine - 1);
    const to = vertexAfterLine(analysis, cursorLine);
    if (from === to) return null;
    const p = pathData(analysis, from, to, preview.map);
    return p.rapid + p.feed;
  }, [preview, analysis, cursorLine]);
  const problems =
    analysis && offset && travel ? fitProblems(analysis, offset, travel) : [];

  return (
    <div className="flex h-full flex-col">
      <div className="flex h-11 shrink-0 items-center gap-3 border-b px-3">
        <div className="font-mono text-xs">
          {hover && (
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
      </div>

      <div ref={area} className="relative flex min-h-0 flex-1 items-center justify-center p-4">
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
            </DropdownMenuContent>
          )}
        </DropdownMenu>
        <svg
          ref={svg}
          viewBox={`${-MARGIN} ${-MARGIN} ${w + 2 * MARGIN} ${h + 2 * MARGIN}`}
          className={cn("h-full max-h-full w-full max-w-full", connected && "cursor-crosshair")}
          onMouseMove={(e) => setHover(toMachine(e))}
          onMouseLeave={() => setHover(null)}
          onClick={openMenu}
          onContextMenu={openMenu}
        >
          <rect x={0} y={0} width={w} height={h} className="fill-muted/30 stroke-border" strokeWidth={0.6} />
          {grid}
          {preview && (
            <g className="pointer-events-none" fill="none" strokeLinejoin="round" strokeLinecap="round">
              {preview.box && (
                <rect
                  {...{ x: preview.box.x, y: preview.box.y, width: preview.box.w, height: preview.box.h }}
                  className="stroke-fuchsia-400/30"
                  strokeWidth={0.3}
                  strokeDasharray="1.5 1"
                />
              )}
              <path d={preview.done.rapid} className="stroke-muted-foreground/20" strokeWidth={0.2} strokeDasharray="1 1" />
              <path d={preview.done.feed} className="stroke-fuchsia-400/25" strokeWidth={0.4} />
              <path d={preview.rest.rapid} className="stroke-muted-foreground/50" strokeWidth={0.2} strokeDasharray="1 1" />
              <path d={preview.rest.feed} className="stroke-fuchsia-400" strokeWidth={0.4} />
              {highlight && <path d={highlight} className="stroke-yellow-300" strokeWidth={1} />}
            </g>
          )}
          {probe && <ProbeOverlay probe={probe} sx={sx} sy={sy} h={h} w={w} />}
          {tool && t && probe && !probe.running && isProbe(status?.tool?.number) && state === "idle" && (
            <ProbePreview form={probeForm} at={[t.machine[0], t.machine[1]]} ball={probe.settings.ballDiameter} sx={sx} sy={sy} />
          )}
          {zero && (
            <g className="stroke-emerald-500" strokeWidth={0.6}>
              <line x1={zero.x - 6} y1={zero.y} x2={zero.x + 6} y2={zero.y} />
              <line x1={zero.x} y1={zero.y - 6} x2={zero.x} y2={zero.y + 6} />
              <circle cx={zero.x} cy={zero.y} r={2.5} fill="none" />
            </g>
          )}
          {hover && connected && !menu && (
            <g className="pointer-events-none stroke-sky-400/50" strokeWidth={0.3}>
              <line x1={sx(hover.x)} y1={0} x2={sx(hover.x)} y2={h} />
              <line x1={0} y1={sy(hover.y)} x2={w} y2={sy(hover.y)} />
            </g>
          )}
          {menu && (
            <circle
              cx={sx(menu.p.x)}
              cy={sy(menu.p.y)}
              r={2}
              fill="none"
              className="pointer-events-none stroke-sky-400"
              strokeWidth={0.6}
            />
          )}
          {target && (
            <g className="stroke-amber-400" strokeWidth={0.6}>
              {tool && <line x1={tool.x} y1={tool.y} x2={sx(target.x)} y2={sy(target.y)} strokeDasharray="2 1.5" />}
              <circle cx={sx(target.x)} cy={sy(target.y)} r={2} fill="none" />
            </g>
          )}
          {tool && (
            <g>
              <circle cx={tool.x} cy={tool.y} r={3.2} className="fill-sky-500/30 stroke-sky-400" strokeWidth={0.6} />
              <circle cx={tool.x} cy={tool.y} r={0.8} className="fill-sky-400" />
            </g>
          )}
          <text x={w / 2} y={h + 6} textAnchor="middle" className="fill-muted-foreground text-[4px]">
            front
          </text>
        </svg>
      </div>
    </div>
  );
}
