import { ask, save } from "@tauri-apps/plugin-dialog";
import { Download, FileCode, Grid3x3, Pause, Play, X } from "lucide-react";
import { useMemo, useState } from "react";

import type { ExportFormat } from "@/bindings/ExportFormat";
import type { SurfaceMap } from "@/bindings/SurfaceMap";
import { Field } from "@/components/Field";
import { Button } from "@/components/ui/button";
import { Progress } from "@/components/ui/progress";
import { ToggleGroup, ToggleGroupItem } from "@/components/ui/toggle-group";
import { act } from "@/lib/actions";
import { useWorkOffset } from "@/lib/machine";
import { useProgram } from "@/lib/program";
import { useStore } from "@/lib/store";
import { duration, heatCss, plannedSpec, pointsRange, remainingSecs, surfacePoints } from "@/lib/surfaceMap";
import { setProbeForm, useUi } from "@/lib/ui";

const FORMATS: { format: ExportFormat; label: string; ext: string; name: string }[] = [
  { format: "csv", label: "CSV", ext: "csv", name: "CSV points" },
  { format: "tsv", label: "TSV", ext: "tsv", name: "TSV points" },
  { format: "stl", label: "STL", ext: "stl", name: "STL mesh" },
];

/** The Map tab of the probe card: the area and grid, starting a scan, and the map it made. */
export function MapTab({ canStart }: { canStart: boolean }) {
  const form = useUi((s) => s.probe);
  const map = useStore((s) => s.state?.probe?.map ?? null);
  const running = useStore((s) => s.state?.probe?.running ?? null);
  const cut = useProgram((s) => s.analysis?.cutBounds ?? null);
  const offset = useWorkOffset();
  const spec = plannedSpec(form, offset ? [offset[0], offset[1]] : null);
  const mapping = running !== null && map !== null && running.label === "Surface map";

  const setCorner = (which: "mapMin" | "mapMax", axis: 0 | 1, v: number) => {
    const p: [number, number] = [...(form[which] ?? form.mapMin ?? form.mapMax ?? [0, 0])] as [number, number];
    p[axis] = v;
    setProbeForm({ [which]: p });
  };

  const start = async () => {
    if (!spec) return;
    if (map && map.probed.length > 0) {
      const ok = await ask(`The current map's ${map.probed.length} points are lost unless you've exported them.`, {
        title: "Start a new map?",
        kind: "warning",
        okLabel: "Start new map",
      });
      if (!ok) return;
    }
    await act({ type: "mapSurface", spec });
  };

  return (
    <div className="space-y-3">
      <div className="flex gap-3">
        <Grid3x3 className="mt-0.5 size-8 shrink-0 text-sky-400" strokeWidth={1.25} />
        <p className="text-xs leading-relaxed text-muted-foreground">
          Drag on the map to pick the area. Then jog the probe above the highest point in it, and anything within a body
          width around it: that's where the scan starts and the highest it goes. It touches down at each point, lifting only
          a little between them, and never takes the probe body below the surface around a point.
        </p>
      </div>

      <div className="grid grid-cols-[auto_auto_auto] items-center justify-start gap-x-2 gap-y-1.5">
        {(["X", "Y"] as const).map((axis, i) => (
          <div key={axis} className="contents">
            <Field label={axis} signed value={form.mapMin?.[i] ?? null} onChange={(v) => setCorner("mapMin", i as 0 | 1, v)} unit="" title={`Work ${axis} where the area starts`} />
            <Field label="to" signed value={form.mapMax?.[i] ?? null} onChange={(v) => setCorner("mapMax", i as 0 | 1, v)} title={`Work ${axis} where the area ends`} />
            {i === 0 ? (
              <Button
                size="xs"
                variant="ghost"
                disabled={!cut}
                title="The area the program in the editor cuts"
                onClick={() => cut && setProbeForm({ mapMin: [cut.min[0], cut.min[1]], mapMax: [cut.max[0], cut.max[1]] })}
              >
                <FileCode /> Program's
              </Button>
            ) : (
              <span />
            )}
          </div>
        ))}
      </div>

      <div className="flex flex-wrap items-center gap-x-3 gap-y-1.5">
        <Field label="Spacing" value={form.mapSpacing} onChange={(mapSpacing) => setProbeForm({ mapSpacing })} title="Between points; rounded so they fit the area evenly" />
        <Field label="Depth" value={form.mapDepth} onChange={(mapDepth) => setProbeForm({ mapDepth })} title="How far below the start height to search at each point" />
        <ToggleGroup
          type="single"
          variant="outline"
          size="sm"
          spacing={0}
          value={form.mapPrecise ? "precise" : "one"}
          onValueChange={(v) => v && setProbeForm({ mapPrecise: v === "precise" })}
        >
          <ToggleGroupItem value="one" className="h-7 px-2 text-xs" title="One touch per point, at the fast feed">
            One touch
          </ToggleGroupItem>
          <ToggleGroupItem value="precise" className="h-7 px-2 text-xs" title="A search, then a slow measuring touch: about twice as long">
            Precise
          </ToggleGroupItem>
        </ToggleGroup>
      </div>

      <div className="flex flex-wrap items-center gap-x-3 gap-y-1.5">
        <ToggleGroup
          type="single"
          variant="outline"
          size="sm"
          spacing={0}
          value={form.mapAdaptive ? "adaptive" : "grid"}
          onValueChange={(v) => v && setProbeForm({ mapAdaptive: v === "adaptive" })}
        >
          <ToggleGroupItem value="grid" className="h-7 px-2 text-xs" title="Every point of the grid, nothing more">
            Grid
          </ToggleGroupItem>
          <ToggleGroupItem
            value="adaptive"
            className="h-7 px-2 text-xs"
            title="The grid, then finer where the surface bends or an edge runs through: each cell's centre is probed, and where it's off its corners' average by more than the tolerance, the cell is split in four, down to the finest spacing"
          >
            Adaptive
          </ToggleGroupItem>
        </ToggleGroup>
        {form.mapAdaptive && (
          <>
            <Field
              label="Halve"
              unit="×"
              value={form.mapLevels}
              onChange={(v) => setProbeForm({ mapLevels: Math.min(Math.max(Math.round(v), 1), 6) })}
              title="How many times the spacing may be halved where the surface needs it (1–6)"
            />
            <Field
              label="Tolerance"
              value={form.mapTolerance}
              onChange={(mapTolerance) => setProbeForm({ mapTolerance })}
              title="How far a cell's centre may be from what its corners predict before the cell is split"
            />
          </>
        )}
      </div>
      {form.mapAdaptive && spec && (
        <p className="-mt-1 text-xs text-muted-foreground">
          Down to {((spec.max[0] - spec.min[0]) / (spec.columns - 1) / 2 ** spec.levels).toFixed(2)} mm where needed.
        </p>
      )}

      {!mapping && (
        <Button className="w-full" disabled={!canStart || !spec} onClick={() => void start()}>
          <Grid3x3 />
          {spec
            ? `Map ${spec.columns} × ${spec.rows} = ${spec.columns * spec.rows} points${spec.levels > 0 ? ", then refine" : ""}`
            : "Pick an area to map"}
        </Button>
      )}

      {map && <MapResult map={map} mapping={mapping} canResume={canStart} offsetZ={offset?.[2] ?? null} />}
    </div>
  );
}

function MapResult({ map, mapping, canResume, offsetZ }: { map: SurfaceMap; mapping: boolean; canResume: boolean; offsetZ: number | null }) {
  const [format, setFormat] = useState<ExportFormat>("csv");
  const compensate = useUi((s) => s.mapCompensate);
  const ball = useStore((s) => s.state?.probe?.settings.ballDiameter ?? 0);
  const done = map.probed.length;
  const complete = map.complete;
  // Refinement plans in batches, so this is what's known so far.
  const total = done + map.pending;
  const adaptive = map.spec.levels > 0;
  // Of the surface as shown (and exported): compensated or not.
  const range = useMemo(() => pointsRange(surfacePoints(map, ball / 2, compensate)), [map, ball, compensate]);
  const found = map.z.filter((z) => z !== null).length;
  const left = remainingSecs(map);
  const work = (z: number) => (offsetZ === null ? "—" : (z - offsetZ).toFixed(3));

  const exportMap = async () => {
    const f = FORMATS.find((f) => f.format === format)!;
    const path = await save({ defaultPath: `surface-map.${f.ext}`, filters: [{ name: f.name, extensions: [f.ext] }] });
    if (path) await act({ type: "exportMap", path, format, compensate });
  };

  return (
    <div className="space-y-2 rounded-md border p-2">
      <div className="flex items-center gap-2">
        <div className="flex-1 text-sm font-medium">
          Surface map
          <span className="ml-2 text-xs font-normal text-muted-foreground">
            {complete
              ? adaptive
                ? `${done} points`
                : `${map.spec.columns} × ${map.spec.rows}`
              : `${mapping ? "" : "paused at "}${done} of ${total}${adaptive ? " so far" : ""}`}
          </span>
        </div>
        {!mapping && (
          <Button size="icon" variant="ghost" className="-my-1 size-6 text-muted-foreground" title="Discard the map" onClick={() => act({ type: "discardMap" })}>
            <X />
          </Button>
        )}
      </div>

      {!complete && (
        <div className="space-y-1">
          <Progress value={total > 0 ? (done / total) * 100 : 0} />
          <div className="flex items-center gap-2 text-xs text-muted-foreground">
            <span className="flex-1">
              {left === null ? "Timing the first point…" : adaptive ? `About ${duration(left)} for those; refining may add more` : `About ${duration(left)} left`}
            </span>
            {mapping ? (
              <Button size="xs" variant="outline" onClick={() => act({ type: "pauseMap" })} title="Stops after this point, at the start height">
                <Pause /> Pause
              </Button>
            ) : (
              <Button size="xs" variant="secondary" disabled={!canResume} onClick={() => act({ type: "resumeMap" })} title="Carries on from the next point; starts by going up to the start height">
                <Play /> Resume
              </Button>
            )}
          </div>
        </div>
      )}

      {range && (
        <div className="grid grid-cols-[auto_1fr] gap-x-3 font-mono text-xs select-text">
          <span className="text-muted-foreground">Low</span>
          <span className="flex items-center justify-end gap-1.5">
            <span className="size-2 rounded-full" style={{ background: heatCss(0) }} />
            {work(range[0])}
          </span>
          <span className="text-muted-foreground">High</span>
          <span className="flex items-center justify-end gap-1.5">
            <span className="size-2 rounded-full" style={{ background: heatCss(1) }} />
            {work(range[1])}
          </span>
          <span className="text-muted-foreground" title="Highest minus lowest point: the flatness, if it's meant to be flat and level">
            Range
          </span>
          <span className="text-right">{(range[1] - range[0]).toFixed(3)}</span>
          {found + map.unreached.length < done && (
            <>
              <span className="text-muted-foreground">Missed</span>
              <span className="text-right" title="Points with no contact within the depth">
                {done - found - map.unreached.length}
              </span>
            </>
          )}
          {map.unreached.length > 0 && (
            <>
              <span className="text-amber-500">Out of reach</span>
              <span
                className="text-right text-amber-500"
                title="Points it stopped short of: going deeper would have taken the probe body below the surface around them (holes and pockets deeper than the stylus)"
              >
                {map.unreached.length}
              </span>
            </>
          )}
        </div>
      )}

      {found > 0 && (
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
            title="Corrects each point for the ball touching a slope off its bottom, using the neighbouring points: on the maps, in these figures and in the export. Off: the tool tip under each grid point."
          >
            <input type="checkbox" className="accent-sky-500" checked={compensate} onChange={(e) => useUi.setState({ mapCompensate: e.target.checked })} />
            Ball compensation
          </label>
          <Button size="sm" variant="outline" className="ml-auto" onClick={() => void exportMap()} title="In the current work coordinates">
            <Download /> Export
          </Button>
        </div>
      )}
    </div>
  );
}
