import { ask, save } from "@tauri-apps/plugin-dialog";
import { Crosshair, Download, FileCode, FileInput, LoaderCircle, Magnet, Play, Spline, X } from "lucide-react";
import { toast } from "sonner";
import { useMemo, useState } from "react";

import type { Contour } from "@/bindings/Contour";
import type { ExportFormat } from "@/bindings/ExportFormat";
import { Field } from "@/components/Field";
import { Button } from "@/components/ui/button";
import { ToggleGroup, ToggleGroupItem } from "@/components/ui/toggle-group";
import { act } from "@/lib/actions";
import { measure, plannedContour, wallPoints } from "@/lib/contour";
import { clearDrawing, openDrawing, runFit, showFit, useDrawing, type V2 } from "@/lib/drawing";
import { useWorkOffset } from "@/lib/machine";
import { useProgram } from "@/lib/program";
import { useStore } from "@/lib/store";
import { startRotate } from "@/lib/transform";
import { setProbeForm, useUi } from "@/lib/ui";
import { cn } from "@/lib/utils";

const FORMATS: { format: ExportFormat; label: string; ext: string; name: string }[] = [
  { format: "dxf", label: "DXF", ext: "dxf", name: "DXF outline" },
  { format: "csv", label: "CSV", ext: "csv", name: "CSV points" },
  { format: "tsv", label: "TSV", ext: "tsv", name: "TSV points" },
];

/** The Contour tab of the probe card: what to trace, starting it, and the contour it made. */
export function ContourTab({ canStart }: { canStart: boolean }) {
  const form = useUi((s) => s.probe);
  const contour = useStore((s) => s.state?.probe?.contour ?? null);
  const running = useStore((s) => s.state?.probe?.running ?? null);
  const cut = useProgram((s) => s.analysis?.cutBounds ?? null);
  const offset = useWorkOffset();
  const spec = plannedContour(form, offset ? [offset[0], offset[1]] : null);
  const tracing = running !== null && contour !== null && !contour.closed;

  const setCorner = (which: "contourMin" | "contourMax", axis: 0 | 1, v: number) => {
    const p = [...(form[which] ?? form.contourMin ?? form.contourMax ?? [0, 0])] as [number, number];
    p[axis] = v;
    setProbeForm({ [which]: p });
  };

  const start = async () => {
    if (!spec) return;
    if (contour?.closed) {
      const ok = await ask("The current contour is lost unless you've exported it.", {
        title: "Trace a new contour?",
        kind: "warning",
        okLabel: "Trace new contour",
      });
      if (!ok) return;
    }
    await act({ type: "traceContour", spec });
  };

  return (
    <div className="space-y-3">
      <ToggleGroup
        type="single"
        variant="outline"
        size="sm"
        spacing={0}
        value={form.contourInside ? "inside" : "outside"}
        onValueChange={(v) => v && setProbeForm({ contourInside: v === "inside" })}
      >
        <ToggleGroupItem value="outside" className="h-7 px-2 text-xs" title="A part's outline, from outside it">
          Outside
        </ToggleGroupItem>
        <ToggleGroupItem value="inside" className="h-7 px-2 text-xs" title="A pocket's or bore's wall, from inside it">
          Inside
        </ToggleGroupItem>
      </ToggleGroup>

      <div className="flex gap-3">
        <Spline className="mt-0.5 size-8 shrink-0 text-sky-400" strokeWidth={1.25} />
        <p className="text-xs leading-relaxed text-muted-foreground">
          {form.contourInside
            ? "Jog the probe into the pocket, at the height to trace and clear of its walls. It finds the front (−Y) wall, then goes round it touching every few millimetres until it's back where it started."
            : "Drag a box around the part on the map. Then jog the probe beside the part, at the height to trace: it moves toward the middle of the box until it touches, then goes round the outline (staying inside the box) until it's back where it started."}
        </p>
      </div>

      {!form.contourInside && (
        <div className="grid grid-cols-[auto_auto_auto] items-center justify-start gap-x-2 gap-y-1.5">
          {(["X", "Y"] as const).map((axis, i) => (
            <div key={axis} className="contents">
              <Field label={axis} signed unit="" value={form.contourMin?.[i] ?? null} onChange={(v) => setCorner("contourMin", i as 0 | 1, v)} title={`Work ${axis} where the box starts`} />
              <Field label="to" signed value={form.contourMax?.[i] ?? null} onChange={(v) => setCorner("contourMax", i as 0 | 1, v)} title={`Work ${axis} where the box ends`} />
              {i === 0 ? (
                <Button
                  size="xs"
                  variant="ghost"
                  disabled={!cut}
                  title="Around the area the program in the editor cuts, with 10 mm to spare"
                  onClick={() => cut && setProbeForm({ contourMin: [cut.min[0] - 10, cut.min[1] - 10], contourMax: [cut.max[0] + 10, cut.max[1] + 10] })}
                >
                  <FileCode /> Program's
                </Button>
              ) : (
                <span />
              )}
            </div>
          ))}
        </div>
      )}

      <div className="flex flex-wrap items-center gap-x-3 gap-y-1.5">
        <Field label="Spacing" value={form.contourSpacing} onChange={(contourSpacing) => setProbeForm({ contourSpacing })} title="Between touches along the wall; finer round corners" />
        <Field
          label="Inside radius"
          value={form.contourRadius}
          onChange={(contourRadius) => setProbeForm({ contourRadius })}
          title={
            form.contourInside
              ? "The tightest the pocket's wall bends toward the probe (its corners: usually the end mill's radius). Tighter only slows it down."
              : "The tightest inside corner of the outline. Tighter only slows it down."
          }
        />
        {form.contourInside && (
          <Field label="Search" value={form.contourSearch} onChange={(contourSearch) => setProbeForm({ contourSearch })} title="How far toward −Y to look for the first wall" />
        )}
      </div>

      {!tracing && (
        <Button className="w-full" disabled={!canStart || !spec} onClick={() => void start()}>
          <Spline />
          {spec ? `Trace ${form.contourInside ? "inside" : "outside"} contour` : "Draw a box around the part"}
        </Button>
      )}

      {contour && contour.points.length > 0 && <ContourResult contour={contour} tracing={tracing} canResume={canStart} />}
    </div>
  );
}

function ContourResult({ contour, tracing, canResume }: { contour: Contour; tracing: boolean; canResume: boolean }) {
  const [format, setFormat] = useState<ExportFormat>("dxf");
  const compensate = useUi((s) => s.mapCompensate);
  const ball = useStore((s) => s.state?.probe?.settings.ballDiameter ?? 0);
  const shape = useMemo(
    () => (contour.closed ? measure(wallPoints(contour.points as [number, number][], ball / 2, compensate, true)) : null),
    [contour, ball, compensate],
  );

  const exportContour = async () => {
    const f = FORMATS.find((f) => f.format === format)!;
    const path = await save({ defaultPath: `contour.${f.ext}`, filters: [{ name: f.name, extensions: [f.ext] }] });
    if (path) await act({ type: "exportContour", path, format, compensate });
  };

  return (
    <div className="space-y-2 rounded-md border p-2">
      <div className="flex items-center gap-2">
        <div className="flex-1 text-sm font-medium">
          {contour.spec.inside ? "Inside contour" : "Outside contour"}
          <span className="ml-2 text-xs font-normal text-muted-foreground">
            {contour.points.length} points{tracing ? ", tracing…" : contour.closed ? "" : ", not closed"}
          </span>
        </div>
        {!tracing && !contour.closed && (
          <Button
            size="xs"
            variant="secondary"
            disabled={!canResume}
            onClick={() => act({ type: "resumeContour" })}
            title="Carries on from the last touches, once the probe is free (Recover) and back at the height it was tracing at"
          >
            <Play /> Resume
          </Button>
        )}
        {!tracing && (
          <Button size="icon" variant="ghost" className="-my-1 size-6 text-muted-foreground" title="Discard the contour" onClick={() => act({ type: "discardContour" })}>
            <X />
          </Button>
        )}
      </div>

      {shape && (
        <div className="grid grid-cols-[auto_1fr] gap-x-3 font-mono text-xs select-text">
          <span className="text-muted-foreground">Perimeter</span>
          <span className="text-right">{shape.perimeter.toFixed(2)} mm</span>
          <span className="text-muted-foreground">Area</span>
          <span className="text-right">{shape.area.toFixed(1)} mm²</span>
        </div>
      )}

      {contour.closed && (
        <div className="flex flex-wrap items-center gap-2">
          <ToggleGroup type="single" variant="outline" size="sm" spacing={0} value={format} onValueChange={(v) => v && setFormat(v as ExportFormat)}>
            {FORMATS.map((f) => (
              <ToggleGroupItem key={f.format} value={f.format} className="h-7 px-2 text-xs" title={f.name}>
                {f.label}
              </ToggleGroupItem>
            ))}
          </ToggleGroup>
          <label
            className="flex items-center gap-1.5 text-xs text-muted-foreground"
            title="Moves each point from where the ball's centre was onto the wall, a ball radius toward the material: on the maps, in these figures and in the export. Inside corners tighter than the ball come out rounded."
          >
            <input type="checkbox" className="accent-sky-500" checked={compensate} onChange={(e) => useUi.setState({ mapCompensate: e.target.checked })} />
            Ball compensation
          </label>
          <Button size="sm" variant="outline" className="ml-auto" onClick={() => void exportContour()} title="In the current work coordinates, at the height it was traced">
            <Download /> Export
          </Button>
        </div>
      )}

      {contour.closed && <DrawingFit contour={contour} />}
    </div>
  );
}

/** Fitting a drawing's outline to the contour: picking the drawing and the outline, the fit, and
 * making the drawing's origin and angle the work origin and the program's rotation. */
function DrawingFit({ contour }: { contour: Contour }) {
  const c = useDrawing((s) => s.current);
  const ball = useStore((s) => s.state?.probe?.settings.ballDiameter ?? 0);
  const offset = useWorkOffset();
  const canAct = useStore((s) => s.state?.status?.state === "idle" && !s.state.job);

  if (!c) {
    return (
      <Button size="sm" variant="outline" className="w-full" onClick={() => void openDrawing()} title="Find where a part drawn in a DXF sits, and how it's turned, from this contour">
        <FileInput /> Fit a drawing…
      </Button>
    );
  }
  const fit = c.fits?.[c.shown] ?? null;
  // The others that fit about as well (a symmetrical part).
  const alike = c.fits?.filter((f) => f.rms < Math.max(2 * (c.fits?.[0].rms ?? 0), 0.05)).length ?? 0;
  const work = (v: number, i: number) => (offset ? (v - offset[i]).toFixed(3) : "—");
  const apply = async () => {
    if (!fit) return;
    if (await act({ type: "setOrigin", x: fit.origin[0], y: fit.origin[1] })) {
      startRotate(Math.round(fit.angle * 1000) / 1000);
      toast("Work origin set at the drawing's origin. Apply the rotation in the Transform sidebar to turn the program with it.");
    }
  };

  return (
    <div className="space-y-2 border-t pt-2">
      <div className="flex items-center gap-2 text-xs">
        <FileInput className="size-3.5 shrink-0 text-violet-400" />
        <span className="min-w-0 flex-1 truncate font-medium" title={c.name}>
          {c.name}
        </span>
        <span className="shrink-0 text-muted-foreground">{c.drawing.units}</span>
        <Button size="icon" variant="ghost" className="-my-1 size-6 text-muted-foreground" title="Put the drawing away" onClick={clearDrawing}>
          <X />
        </Button>
      </div>
      {c.drawing.skipped.length > 0 && <p className="text-xs text-amber-500">Not read: {c.drawing.skipped.join(", ")}.</p>}
      {!fit && (
        <>
          <p className="text-xs text-muted-foreground">
            {c.selected === null ? "Click the outline on the map that matches the trace." : "Fitting the highlighted outline: click another on the map to change it."}
          </p>
          <Button
            size="sm"
            className="w-full"
            disabled={c.selected === null || c.fitting}
            onClick={() => void runFit(contour.points as V2[], contour.spec.inside ? -ball / 2 : ball / 2)}
            title="Turns and moves the outline to lie on the traced wall: where the ball's centre was, a ball radius off the outline (whatever the compensation setting)"
          >
            {c.fitting ? <LoaderCircle className="animate-spin" /> : <Magnet />} Fit to the contour
          </Button>
        </>
      )}
      {fit && (
        <>
          <div className="grid grid-cols-[auto_1fr] gap-x-3 font-mono text-xs select-text">
            <span className="text-muted-foreground">Turned</span>
            <span className="text-right">
              {fit.angle.toFixed(3)}° {fit.angle >= 0 ? "⟲" : "⟳"}
            </span>
            <span className="text-muted-foreground" title="Where the drawing's origin is, in work coordinates now">
              Origin
            </span>
            <span className="text-right">
              X{work(fit.origin[0], 0)} Y{work(fit.origin[1], 1)}
            </span>
            <span className="text-muted-foreground" title="How far the traced points are from the fitted outline: typical and worst">
              Off by
            </span>
            <span className={cn("text-right", fit.max > 0.2 && "text-amber-500")}>
              {fit.rms.toFixed(3)} · max {fit.max.toFixed(3)} mm
            </span>
          </div>
          {fit.max > 0.2 && (
            <p className="text-xs text-amber-500">
              Doesn't match closely: is it the right outline (inside or outside), and the right way up?
            </p>
          )}
          {alike > 1 && (
            <div className="flex items-center gap-2 text-xs text-muted-foreground">
              <span className="flex-1">It fits as well {alike - 1 === 1 ? "another way" : `${alike - 1} other ways`}: it's symmetrical.</span>
              <Button size="xs" variant="outline" onClick={() => showFit((c.shown + 1) % alike)}>
                Next ({c.shown + 1} of {alike})
              </Button>
            </div>
          )}
          <div className="flex gap-2">
            <Button size="sm" className="flex-1" disabled={!canAct} onClick={() => void apply()} title="Sets the work X/Y origin at the drawing's origin and opens the rotation for the program">
              <Crosshair /> Use as origin
            </Button>
            <Button size="sm" variant="ghost" onClick={() => useDrawing.setState((s) => (s.current ? { current: { ...s.current, fits: null } } : {}))}>
              Refit
            </Button>
          </div>
        </>
      )}
    </div>
  );
}
