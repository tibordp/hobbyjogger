import { ArrowDownToLine, CircleDot, Crosshair, LoaderCircle, Navigation, RotateCcwSquare, Settings2, Square, TriangleAlert, Undo2, X } from "lucide-react";
import { useState } from "react";

import type { Across } from "@/bindings/Across";
import type { Corner } from "@/bindings/Corner";
import type { ProbeResult } from "@/bindings/ProbeResult";
import type { ProbeSettings } from "@/bindings/ProbeSettings";
import type { Side } from "@/bindings/Side";
import { Button } from "@/components/ui/button";
import { Card, CardContent, CardHeader, CardTitle } from "@/components/ui/card";
import { Field } from "@/components/Field";
import { ContourTab } from "@/components/ContourTab";
import { MapTab } from "@/components/MapTab";
import { ToggleGroup, ToggleGroupItem } from "@/components/ui/toggle-group";
import { act } from "@/lib/actions";
import { DEPTH_PROBE_TOOL, PROBE_TOOL, useConnected, useJobRunning, useMachineState, useStatus } from "@/lib/machine";
import { useStore } from "@/lib/store";
import { startRotate } from "@/lib/transform";
import { routineOf, setProbeForm, useUi, type ProbeForm, type ProbeKind } from "@/lib/ui";
import { cn } from "@/lib/utils";

/** Single-shot routines, then the scans: one control over two rows, as one row doesn't fit them. */
const KIND_ROWS: { kind: ProbeKind; label: string; title?: string }[][] = [
  [
    { kind: "surface", label: "Surface" },
    { kind: "edge", label: "Edge" },
    { kind: "corner", label: "Corner" },
    { kind: "bore", label: "Bore" },
    { kind: "boss", label: "Boss" },
  ],
  [
    { kind: "map", label: "Surface map", title: "Heights over an area, as points or a mesh" },
    { kind: "contour", label: "Contour", title: "A closed outline at one height, inside or out" },
  ],
];
const KINDS = KIND_ROWS.flat();

// ---- diagrams (top view, front at the bottom, except the surface's side view) ----

const PART = { x: 14, y: 14, s: 32 };
const SIDE_LINE: Record<Side, [number, number, number, number]> = {
  left: [PART.x, PART.y, PART.x, PART.y + PART.s],
  right: [PART.x + PART.s, PART.y, PART.x + PART.s, PART.y + PART.s],
  back: [PART.x, PART.y, PART.x + PART.s, PART.y],
  front: [PART.x, PART.y + PART.s, PART.x + PART.s, PART.y + PART.s],
};
/** Where to start for each edge: beside the part, outside it. */
const SIDE_START: Record<Side, [number, number]> = {
  left: [PART.x - 8, 30],
  right: [PART.x + PART.s + 8, 30],
  back: [30, PART.y - 8],
  front: [30, PART.y + PART.s + 8],
};
const CORNER_AT: Record<Corner, [number, number]> = {
  frontLeft: [PART.x, PART.y + PART.s],
  frontRight: [PART.x + PART.s, PART.y + PART.s],
  backLeft: [PART.x, PART.y],
  backRight: [PART.x + PART.s, PART.y],
};

function Start({ at }: { at: [number, number] }) {
  return <circle cx={at[0]} cy={at[1]} r={2.6} className="fill-sky-400" />;
}

function Diagram({ form }: { form: ProbeForm }) {
  const part = <rect {...{ x: PART.x, y: PART.y, width: PART.s, height: PART.s }} rx={1.5} className="fill-muted stroke-muted-foreground/60" />;
  const pick = "cursor-pointer stroke-transparent hover:stroke-sky-400/40";
  return (
    <svg viewBox="0 0 60 60" className="size-24 shrink-0">
      {form.kind === "surface" && (
        <>
          <rect x={8} y={38} width={44} height={14} rx={1.5} className="fill-muted stroke-muted-foreground/60" />
          <line x1={30} y1={14} x2={30} y2={33} className="stroke-sky-400" strokeWidth={1.2} strokeDasharray="2 1.5" />
          <path d="M26.5 30 L30 35 L33.5 30" className="fill-none stroke-sky-400" strokeWidth={1.2} />
          <Start at={[30, 11]} />
        </>
      )}
      {form.kind === "edge" && (
        <>
          {part}
          {(Object.keys(SIDE_LINE) as Side[]).map((side) => (
            <line
              key={side}
              {...lineProps(SIDE_LINE[side])}
              strokeWidth={6}
              className={pick}
              onClick={() => setProbeForm({ side })}
            />
          ))}
          <line {...lineProps(SIDE_LINE[form.side])} strokeWidth={2} className="pointer-events-none stroke-sky-400" />
          {(() => {
            const [x, y] = SIDE_START[form.side];
            const [ex, ey] = [x + (30 - x) * 0.25, y + (30 - y) * 0.25];
            return <line x1={x} y1={y} x2={ex} y2={ey} className="pointer-events-none stroke-sky-400" strokeWidth={1.2} strokeDasharray="2 1.5" />;
          })()}
          <Start at={SIDE_START[form.side]} />
        </>
      )}
      {form.kind === "corner" && (
        <>
          {part}
          {(Object.keys(CORNER_AT) as Corner[]).map((corner) => {
            const [x, y] = CORNER_AT[corner];
            return (
              <circle key={corner} cx={x} cy={y} r={6} className={cn(pick, "fill-transparent")} strokeWidth={4} onClick={() => setProbeForm({ corner })} />
            );
          })}
          {(() => {
            const [x, y] = CORNER_AT[form.corner];
            const dx = x === PART.x ? 1 : -1;
            const dy = y === PART.y ? 1 : -1;
            return (
              <>
                <path d={`M${x} ${y + dy * 12} L${x} ${y} L${x + dx * 12} ${y}`} className="pointer-events-none fill-none stroke-sky-400" strokeWidth={2} />
                <Start at={[x + dx * 7, y + dy * 7]} />
              </>
            );
          })()}
        </>
      )}
      {(form.kind === "bore" || form.kind === "boss") && (
        <>
          {part}
          {(() => {
            const bore = form.kind === "bore";
            const fill = bore ? "fill-background" : "fill-muted-foreground/40";
            // Across one axis it's a slot or a web running along the other one.
            const feature =
              form.across === "xy" ? (
                <circle cx={30} cy={30} r={bore ? 10 : 9} className={cn(fill, "stroke-sky-400")} strokeWidth={1.5} />
              ) : form.across === "x" ? (
                <rect x={bore ? 21 : 22} y={PART.y + 3} width={bore ? 18 : 16} height={PART.s - 6} rx={bore ? 9 : 1} className={cn(fill, "stroke-sky-400")} strokeWidth={1.5} />
              ) : (
                <rect x={PART.x + 3} y={bore ? 21 : 22} width={PART.s - 6} height={bore ? 18 : 16} rx={bore ? 9 : 1} className={cn(fill, "stroke-sky-400")} strokeWidth={1.5} />
              );
            return feature;
          })()}
          {form.across !== "xy" && (
            <line
              {...(form.across === "x" ? { x1: 18, y1: 30, x2: 42, y2: 30 } : { x1: 30, y1: 18, x2: 30, y2: 42 })}
              className="stroke-sky-400"
              strokeWidth={1.2}
              strokeDasharray="2 1.5"
            />
          )}
          <Start at={[30, 30]} />
        </>
      )}
    </svg>
  );
}

const ACROSS: [Across, string][] = [
  ["xy", "XY"],
  ["x", "X"],
  ["y", "Y"],
];

const lineProps = ([x1, y1, x2, y2]: [number, number, number, number]) => ({ x1, y1, x2, y2 });

const SIDE_TEXT: Record<Side, string> = { left: "left", right: "right", front: "front", back: "back" };
const CORNER_TEXT: Record<Corner, string> = {
  frontLeft: "front left",
  frontRight: "front right",
  backLeft: "back left",
  backRight: "back right",
};

function instructions(f: ProbeForm) {
  switch (f.kind) {
    case "surface":
      return "Jog the probe above the surface. It touches down and finds the top.";
    case "edge":
      return f.edgeAngle
        ? `Jog the probe beside the part, outside the ${SIDE_TEXT[f.side]} edge, at the height to touch it and level with the middle of it. It touches twice, ${f.edgeSpan} mm apart along the edge, for its angle too.`
        : `Jog the probe beside the part, outside the ${SIDE_TEXT[f.side]} edge, at the height to touch it. It moves in until it touches (click an edge to pick another).`;
    case "corner":
      return `Jog the probe above the part, within ${f.reach} mm of both edges at the ${CORNER_TEXT[f.corner]} corner (click a corner to pick another).`;
    case "bore":
      return f.across === "xy"
        ? "Jog the probe into the bore, below its top, roughly in the middle."
        : `Jog the probe into the slot, below its top, roughly in the middle. It finds the centreline across ${f.across.toUpperCase()}.`;
    case "boss":
      return f.across === "xy"
        ? "Jog the probe above the boss, roughly over its centre."
        : `Jog the probe above the boss or web, roughly over its middle. It finds the centreline across ${f.across.toUpperCase()}.`;
    case "map":
    case "contour":
      return "";
  }
}

function Settings({ settings }: { settings: ProbeSettings }) {
  const set = (patch: Partial<ProbeSettings>) => act({ type: "setProbeSettings", settings: { ...settings, ...patch } });
  return (
    <div className="grid grid-cols-2 gap-x-3 gap-y-1.5 rounded-md bg-muted/40 p-2">
      <Field label="Ball Ø" value={settings.ballDiameter} onChange={(ballDiameter) => set({ ballDiameter })} />
      <Field label="Clearance" value={settings.clearance} onChange={(clearance) => set({ clearance })} title="Height above the top for moves over the part" />
      <Field label="Fast" value={settings.fastFeed} onChange={(fastFeed) => set({ fastFeed })} title="Searching touch, mm/min" />
      <Field label="Slow" value={settings.slowFeed} onChange={(slowFeed) => set({ slowFeed })} title="Measuring touch, mm/min" />
      <Field label="Travel" value={settings.travelFeed} onChange={(travelFeed) => set({ travelFeed })} title="Moves between touches (they stop on contact too), mm/min" />
      <Field label="Z search" value={settings.zSearch} onChange={(zSearch) => set({ zSearch })} title="How far down to look for the top" />
      <Field label="Stylus" value={settings.stylusLength} onChange={(stylusLength) => set({ stylusLength })} title="3D probe: from the ball's bottom to the probe body. Maps keep the body this far (less 2 mm) above what's around." />
      <Field label="Body Ø" value={settings.bodyDiameter} onChange={(bodyDiameter) => set({ bodyDiameter })} title="3D probe: the body's diameter, for what's around a point when mapping" />
    </div>
  );
}

function Result({ result, offset, canApply }: { result: ProbeResult; offset: number[] | null; canApply: boolean }) {
  const rows: [string, number | null, number][] = [
    ["X", result.x, 0],
    ["Y", result.y, 1],
    ["Z", result.z, 2],
  ];
  const xy = result.x !== null || result.y !== null;
  // Once it's the origin the result has done its job.
  const origin = async (withZ: boolean) => {
    const ok = await act({
      type: "setOrigin",
      x: result.x ?? undefined,
      y: result.y ?? undefined,
      z: withZ ? (result.z ?? undefined) : undefined,
    });
    if (ok) await act({ type: "dismissProbe" });
  };
  const letters = (withZ: boolean) => rows.filter(([a, v]) => v !== null && (withZ || a !== "Z")).map(([a]) => a).join("");
  // A bore or boss: its centre, or across one axis, its centreline.
  const centre = (result.diameter !== null || result.width !== null) && xy;

  return (
    <div className="space-y-2 rounded-md border p-2">
      <div className="flex items-center justify-between">
        <div className="text-sm font-medium">{result.label}</div>
        <Button
          size="icon"
          variant="ghost"
          className="-my-1 size-6 text-muted-foreground"
          onClick={() => act({ type: "dismissProbe" })}
          title="Dismiss"
        >
          <X />
        </Button>
      </div>
      <div className="grid grid-cols-[auto_1fr_auto] gap-x-3 font-mono text-sm select-text">
        {rows
          .filter(([, v]) => v !== null)
          .map(([axis, v, i]) => (
            <div key={axis} className="contents">
              <span className="text-muted-foreground">{axis}</span>
              <span className="text-right">{offset ? (v! - offset[i]).toFixed(3) : "—"}</span>
              <span className="text-right text-xs leading-5 text-muted-foreground">M {v!.toFixed(3)}</span>
            </div>
          ))}
        {result.diameter !== null && (
          <>
            <span className="text-muted-foreground">Ø</span>
            <span className="text-right">{result.diameter.toFixed(3)}</span>
            <span />
          </>
        )}
        {result.width !== null && (
          <>
            <span className="text-muted-foreground" title="Width">W</span>
            <span className="text-right">{result.width.toFixed(3)}</span>
            <span />
          </>
        )}
        {result.angle !== null && (
          <>
            <span className="text-muted-foreground" title="How far the part is turned from square, counter-clockwise seen from above">
              ∠
            </span>
            <span className="text-right">{result.angle.toFixed(3)}°</span>
            <span className="text-right text-xs leading-5 text-muted-foreground">{result.angle >= 0 ? "⟲ CCW" : "⟳ CW"}</span>
          </>
        )}
      </div>
      <div className="flex flex-wrap gap-2">
        <Button size="sm" disabled={!canApply} onClick={() => origin(true)} title="Makes this the work origin. Nothing moves.">
          <Crosshair /> Set {letters(true)} origin
        </Button>
        {xy && result.z !== null && (
          <Button size="sm" variant="outline" disabled={!canApply} onClick={() => origin(false)}>
            {letters(false)} only
          </Button>
        )}
        {result.angle !== null && (
          <Button
            size="sm"
            variant="outline"
            onClick={() => startRotate(Math.round(result.angle! * 1000) / 1000)}
            title="Turns the program in the editor by this, about its X0 Y0, so it lines up with the part"
          >
            <RotateCcwSquare /> Rotate program by this
          </Button>
        )}
        {centre && (
          <Button
            size="sm"
            variant="outline"
            disabled={!canApply}
            onClick={() =>
              act({ type: "moveTo", x: result.x ?? undefined, y: result.y ?? undefined, speed: useUi.getState().jogSpeed })
            }
            title="Moves there at the current height"
          >
            <Navigation /> Go to {result.x !== null && result.y !== null ? "centre" : "centreline"}
          </Button>
        )}
      </div>
    </div>
  );
}

export function ProbePanel() {
  const connected = useConnected();
  const state = useMachineState();
  const job = useJobRunning();
  const status = useStatus();
  const probe = useStore((s) => s.state?.probe ?? null);
  const changing = useStore((s) => s.state?.toolChange != null);
  const form = useUi((s) => s.probe);
  const [showSettings, setShowSettings] = useState(false);

  const hasProbe = status?.tool?.number === PROBE_TOOL;
  // The depth probe only touches down: Surface works with it, nothing else.
  const depthOnly = status?.tool?.number === DEPTH_PROBE_TOOL;
  // Maps only touch down too, but cross at the top with the depth probe (it can't feel the part
  // in the way).
  const canProbe = hasProbe || (depthOnly && (form.kind === "surface" || form.kind === "map"));
  // Contours need to feel sideways: the 3D probe only (covered by hasProbe).
  const running = probe?.running ?? null;
  const idle = connected && state === "idle" && !job && !changing && !running;
  // Nothing to show but how to get a probe in.
  const collapsed = !hasProbe && !depthOnly && !running && !probe?.error && !probe?.result && !probe?.map && !probe?.contour;

  if (collapsed) {
    return (
      <Card size="sm">
        <CardHeader className="flex flex-row items-center gap-2">
          <CardTitle className="flex-1">Probe</CardTitle>
          <span className="text-xs text-muted-foreground">{connected ? "No probe in" : "Not connected"}</span>
        </CardHeader>
        {connected && (
          <CardContent className="grid grid-cols-2 gap-2">
            <Button size="sm" variant="secondary" disabled={!idle} onClick={() => act({ type: "changeTool", tool: PROBE_TOOL })}>
              <CircleDot /> 3D probe
            </Button>
            <Button size="sm" variant="secondary" disabled={!idle} onClick={() => act({ type: "changeTool", tool: DEPTH_PROBE_TOOL })}>
              <ArrowDownToLine /> Depth probe
            </Button>
          </CardContent>
        )}
      </Card>
    );
  }

  return (
    <Card size="sm">
      <CardHeader className="flex flex-row items-center gap-1">
        <CardTitle className="flex-1">Probe</CardTitle>
        <Button size="icon-sm" variant="ghost" title="Probe settings" onClick={() => setShowSettings((v) => !v)}>
          <Settings2 />
        </Button>
      </CardHeader>
      <CardContent className="space-y-3">
        {showSettings && probe && <Settings settings={probe.settings} />}

        {connected && !canProbe && !changing && (
          <div className="flex items-center gap-2 rounded-md border border-amber-500/40 bg-amber-500/10 p-2 text-sm">
            <span className="flex-1">
              {depthOnly ? "The depth probe only measures Z; this needs the 3D probe." : "No probe in."}
            </span>
            <Button size="sm" variant="secondary" disabled={!idle} onClick={() => act({ type: "changeTool", tool: PROBE_TOOL })}>
              <CircleDot /> Switch to it
            </Button>
          </div>
        )}

        {/* A 10-column grid (routines span 2, scans 5); the 1 px gaps show the border colour
            behind, so the inner borders are flat and only the outer corners round. */}
        <ToggleGroup
          type="single"
          size="sm"
          spacing={0}
          className="grid w-full grid-cols-10 gap-px overflow-hidden rounded-md border border-input bg-input"
          value={form.kind}
          onValueChange={(kind) => kind && setProbeForm({ kind: kind as ProbeKind })}
        >
          {KIND_ROWS.map((row) =>
            row.map((k) => (
              <ToggleGroupItem
                key={k.kind}
                value={k.kind}
                title={k.title}
                className={cn(
                  // Important: the group rounds its first and last items with more specific selectors.
                  "rounded-none! bg-card text-xs hover:bg-muted data-[state=on]:bg-muted",
                  row.length === 2 ? "col-span-5" : "col-span-2",
                )}
              >
                {k.label}
              </ToggleGroupItem>
            )),
          )}
        </ToggleGroup>

        {form.kind === "map" ? (
          <MapTab canStart={idle && canProbe} />
        ) : form.kind === "contour" ? (
          <ContourTab canStart={idle && hasProbe} />
        ) : (
          <div className="flex gap-3">
            <Diagram form={form} />
            <div className="space-y-2">
              <p className="text-xs leading-relaxed text-muted-foreground">{instructions(form)}</p>
              <div className="flex flex-wrap gap-x-3 gap-y-1.5">
                {form.kind === "edge" && (
                  <>
                    <ToggleGroup
                      type="single"
                      variant="outline"
                      size="sm"
                      spacing={0}
                      value={form.edgeAngle ? "angle" : "position"}
                      onValueChange={(v) => v && setProbeForm({ edgeAngle: v === "angle" })}
                    >
                      <ToggleGroupItem value="position" className="h-7 px-2 text-xs" title="One touch: where the edge is">
                        Position
                      </ToggleGroupItem>
                      <ToggleGroupItem value="angle" className="h-7 px-2 text-xs" title="Two touches along it: where it is and how it's turned">
                        Angle
                      </ToggleGroupItem>
                    </ToggleGroup>
                    <Field label="Search" value={form.edgeSearch} onChange={(edgeSearch) => setProbeForm({ edgeSearch })} title="How far it moves in looking for the edge" />
                    {form.edgeAngle && (
                      <Field label="Span" value={form.edgeSpan} onChange={(edgeSpan) => setProbeForm({ edgeSpan })} title="How far apart along the edge it touches: further is more precise" />
                    )}
                  </>
                )}
                {form.kind === "corner" && (
                  <Field label="Reach" value={form.reach} onChange={(reach) => setProbeForm({ reach })} title="How far the edges may be from where the probe starts" />
                )}
                {(form.kind === "bore" || form.kind === "boss") && (
                  <ToggleGroup
                    type="single"
                    variant="outline"
                    size="sm"
                    spacing={0}
                    value={form.across}
                    onValueChange={(across) => across && setProbeForm({ across: across as Across })}
                    title="Both axes for a centre, or one for a centreline"
                  >
                    {ACROSS.map(([value, label]) => (
                      <ToggleGroupItem key={value} value={value} className="h-7 px-2 font-mono text-xs">
                        {label}
                      </ToggleGroupItem>
                    ))}
                  </ToggleGroup>
                )}
                {form.kind === "bore" && (
                  <Field label={form.across === "xy" ? "About Ø" : "About width"} value={form.boreDiameter} onChange={(boreDiameter) => setProbeForm({ boreDiameter })} title="Roughly; it searches a little beyond" />
                )}
                {form.kind === "boss" && (
                  <Field label={form.across === "xy" ? "About Ø" : "About width"} value={form.bossDiameter} onChange={(bossDiameter) => setProbeForm({ bossDiameter })} title="Roughly, or a bit more: it goes down outside this" />
                )}
                {(form.kind === "corner" || form.kind === "boss") && (
                  <Field label="Depth" value={form.depth} onChange={(depth) => setProbeForm({ depth })} title="How far below the top to touch the sides" />
                )}
              </div>
            </div>
          </div>

        )}

        {running ? (
          <div className="flex items-center gap-2">
            <LoaderCircle className="size-4 shrink-0 animate-spin text-sky-400" />
            <div className="min-w-0 flex-1 text-sm">
              <div className="truncate">{running.label}</div>
              <div className="truncate text-xs text-muted-foreground">{running.step}…</div>
            </div>
            <Button size="sm" variant="outline" onClick={() => act({ type: "cancelProbe" })} title="Halts the machine and clears the halt">
              <Square /> Stop
            </Button>
          </div>
        ) : (
          form.kind !== "map" && form.kind !== "contour" && (
            <Button className="w-full" disabled={!idle || !canProbe} onClick={() => act({ type: "probe", routine: routineOf(form) })}>
              <CircleDot /> Probe {KINDS.find((k) => k.kind === form.kind)!.label.toLowerCase()}
            </Button>
          )
        )}

        {probe?.error && !running && (
          <div className="space-y-2 rounded-md border border-red-500/40 bg-red-500/10 p-2 text-sm">
            <div className="flex gap-2">
              <TriangleAlert className="mt-0.5 size-4 shrink-0 text-red-400" />
              <span>Probing stopped: {probe.error}</span>
            </div>
            <div className="flex gap-2">
              {(probe.canBackOff || state === "alarm") && (
                <Button
                  size="sm"
                  variant="secondary"
                  onClick={() => act({ type: "recoverProbe" })}
                  title="Clears a halt and backs the probe off the way it came until it releases"
                >
                  <Undo2 /> Recover
                </Button>
              )}
              <Button size="sm" variant="ghost" onClick={() => act({ type: "dismissProbe" })}>
                Dismiss
              </Button>
            </div>
            <p className="text-xs text-muted-foreground">
              The probe's light can stay green for a few seconds after it's free.
            </p>
          </div>
        )}
        {probe?.result && !running && (
          <Result result={probe.result} offset={status?.workOffset ?? null} canApply={idle} />
        )}
      </CardContent>
    </Card>
  );
}
