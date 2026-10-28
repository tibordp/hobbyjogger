import { ChevronRight, TriangleAlert, Wrench } from "lucide-react";
import { useState } from "react";

import { ToolPicker } from "@/components/ToolPicker";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Card, CardContent, CardHeader, CardTitle } from "@/components/ui/card";
import { act } from "@/lib/actions";
import { STATE_LABEL, toolName, useConnected, useJobRunning, useStatus } from "@/lib/machine";
import { useStore } from "@/lib/store";
import { cn } from "@/lib/utils";

const AXES = ["X", "Y", "Z", "A"] as const;

const STATE_STYLE: Record<string, string> = {
  idle: "bg-emerald-600 text-white",
  run: "bg-sky-600 text-white",
  home: "bg-sky-600 text-white",
  alarm: "bg-red-600 text-white",
  hold: "bg-amber-500 text-black",
  pause: "bg-amber-500 text-black",
  wait: "bg-amber-500 text-black",
  tool: "bg-violet-600 text-white",
};

function fmt(v: number | undefined, digits = 3) {
  return v === undefined ? "—" : v.toFixed(digits);
}


type AxisName = (typeof AXES)[number];

/** A work-position value that turns into an input on click, to set the work coordinate. */
function WorkValue({
  axis,
  value,
  editable,
  className,
}: {
  axis: AxisName;
  value: number | undefined;
  editable: boolean;
  className: string;
}) {
  const [draft, setDraft] = useState<string | null>(null);

  if (draft !== null) {
    const commit = async () => {
      const v = Number(draft.trim().replace(",", "."));
      if (draft.trim() === "" || !Number.isFinite(v)) {
        setDraft(null);
        return;
      }
      if (await act({ type: "setWork", [axis.toLowerCase()]: v })) setDraft(null);
    };
    return (
      <input
        autoFocus
        className="w-full rounded-md border border-ring bg-transparent px-1 text-right font-mono text-3xl tabular-nums outline-none"
        value={draft}
        onChange={(e) => setDraft(e.target.value)}
        onFocus={(e) => e.target.select()}
        onBlur={() => setDraft(null)}
        onKeyDown={(e) => {
          if (e.key === "Enter") void commit();
          else if (e.key === "Escape") setDraft(null);
        }}
      />
    );
  }

  return (
    <button
      type="button"
      disabled={!editable}
      title={editable ? `Click to set the work ${axis} coordinate` : undefined}
      className={cn(
        "rounded-md px-1 text-right font-mono text-3xl tabular-nums",
        editable && "cursor-text hover:bg-muted",
        className,
      )}
      onClick={() => setDraft(value !== undefined ? value.toFixed(3) : "0")}
    >
      {value === undefined ? "—" : fmt(value)}
    </button>
  );
}

export function Dro() {
  const connected = useConnected();
  const status = useStatus();
  const job = useJobRunning();
  const t = useStore((s) => s.telemetry);
  const live = t?.live ?? true;
  const editable = connected && status?.state === "idle" && !job;
  const busy = useStore((s) => s.state?.toolChange != null || s.state?.probe?.running != null);
  const [picking, setPicking] = useState(false);

  return (
    <Card size="sm">
      <CardHeader className="flex flex-row items-center justify-between">
        <CardTitle>Position</CardTitle>
        {status && (
          <Badge className={cn("uppercase", STATE_STYLE[status.state])}>
            {STATE_LABEL[status.state]}
          </Badge>
        )}
      </CardHeader>
      <CardContent className="space-y-3">
        <div className="grid grid-cols-[auto_1fr_auto] items-baseline gap-x-3 gap-y-1">
          <span />
          <span className="text-right text-xs text-muted-foreground">work</span>
          <span className="text-right text-xs text-muted-foreground">machine</span>
          {AXES.map((axis, i) => (
            <div key={axis} className="contents">
              <span className="text-lg font-semibold">{axis}</span>
              <WorkValue
                axis={axis}
                value={connected ? t?.work[i] : undefined}
                editable={editable}
                className={cn(!connected && "text-muted-foreground", !live && "text-amber-500")}
              />
              <span className="text-right font-mono text-sm tabular-nums text-muted-foreground select-text">
                {connected ? fmt(t?.machine[i]) : "—"}
              </span>
            </div>
          ))}
        </div>
        {!live && (
          <p className="text-xs text-amber-500">
            Planned end position: the machine doesn't report the live position in this state.
          </p>
        )}

        {status?.halt && (
          <div className="flex items-center gap-2 rounded-md border border-red-600/50 bg-red-600/10 p-2 text-sm">
            <TriangleAlert className="size-4 shrink-0 text-red-500" />
            <div className="flex-1">
              Halted: <b>{status.halt.reason}</b>
              {status.halt.recovery !== "clear" && (
                <div className="text-xs text-muted-foreground">
                  {status.halt.recovery === "reset" ? "Needs a controller reset (Reset, top right)." : "Needs a power cycle."}
                </div>
              )}
            </div>
            <Button size="sm" variant="outline" onClick={() => act({ type: "unlock" })}>
              Unlock ($X)
            </Button>
          </div>
        )}

        {status && (
          <button
            type="button"
            disabled={!editable || busy}
            title={editable && !busy ? "Change the tool" : undefined}
            className="group flex w-full items-center gap-2.5 rounded-lg border px-2.5 py-1.5 text-left enabled:cursor-pointer enabled:hover:bg-muted disabled:opacity-60"
            onClick={() => setPicking(true)}
          >
            <Wrench className="size-4 shrink-0 text-muted-foreground" />
            <span className="min-w-0 flex-1 truncate text-base font-semibold">
              {status.tool ? (
                <>
                  {toolName(status.tool.number)}
                  {status.tool.target != null && <> → {toolName(status.tool.target)}</>}
                </>
              ) : (
                "No tool"
              )}
            </span>
            {status.tool && status.tool.number >= 0 && (
              <span className="text-sm tabular-nums text-muted-foreground" title="Tool length offset, relative to the tool Z was zeroed with">
                {status.tool.offset >= 0 && "+"}
                {status.tool.offset.toFixed(3)}
              </span>
            )}
            <ChevronRight className="size-4 shrink-0 text-muted-foreground group-disabled:hidden" />
          </button>
        )}
        <ToolPicker open={picking} onOpenChange={setPicking} />

        {status && (
          <dl className="grid grid-cols-2 gap-x-4 gap-y-1 text-sm tabular-nums">
            <dt className="text-muted-foreground">Feed</dt>
            <dd className="text-right">
              {fmt(t?.feed, 0)} / {fmt(status.feedRequested ?? undefined, 0)}
            </dd>
            {status.laser?.mode ? (
              <>
                <dt className="text-muted-foreground">Laser</dt>
                <dd className="text-right">
                  {fmt(t?.laserPower ?? undefined, 0)} %
                  {status.laser.testing ? " · test fire" : status.laser.on ? " · on" : ""}
                </dd>
              </>
            ) : (
              <>
                <dt className="text-muted-foreground">Spindle</dt>
                <dd className="text-right">
                  {fmt(t?.spindleRpm, 0)} rpm
                  {t?.spindleTemp != null && <> · {t.spindleTemp.toFixed(0)}°C</>}
                </dd>
              </>
            )}
            {status.job && (
              <>
                <dt className="text-muted-foreground">Job</dt>
                <dd className="text-right">
                  {status.job.percent}% · line {status.job.lines}
                </dd>
              </>
            )}
          </dl>
        )}
      </CardContent>
    </Card>
  );
}
