import { ask, save } from "@tauri-apps/plugin-dialog";
import { Download, FileCode, Spline, X } from "lucide-react";
import { useMemo, useState } from "react";

import type { Contour } from "@/bindings/Contour";
import type { ExportFormat } from "@/bindings/ExportFormat";
import { Field } from "@/components/Field";
import { Button } from "@/components/ui/button";
import { ToggleGroup, ToggleGroupItem } from "@/components/ui/toggle-group";
import { act } from "@/lib/actions";
import { measure, plannedContour, wallPoints } from "@/lib/contour";
import { useWorkOffset } from "@/lib/machine";
import { useProgram } from "@/lib/program";
import { useStore } from "@/lib/store";
import { setProbeForm, useUi } from "@/lib/ui";

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

      {contour && contour.points.length > 0 && <ContourResult contour={contour} tracing={tracing} />}
    </div>
  );
}

function ContourResult({ contour, tracing }: { contour: Contour; tracing: boolean }) {
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
    </div>
  );
}
