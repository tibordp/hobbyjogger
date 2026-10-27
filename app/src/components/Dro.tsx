import { TriangleAlert } from "lucide-react";
import { useState } from "react";

import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Card, CardContent, CardHeader, CardTitle } from "@/components/ui/card";
import { act } from "@/lib/actions";
import { STATE_LABEL, useConnected, useJobRunning, useStatus } from "@/lib/machine";
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

function toolName(n: number) {
  if (n === 9999) return "3D probe";
  if (n < 0) return "none";
  return `T${n}`;
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
        className="w-full rounded-md border border-ring bg-transparent px-1 text-right text-3xl tabular-nums outline-none"
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
        "rounded-md px-1 text-right text-3xl tabular-nums",
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
        <div className="grid grid-cols-[auto_1fr_auto] items-baseline gap-x-3 gap-y-1 font-mono">
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
              <span className="text-right text-sm tabular-nums text-muted-foreground">
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
          <dl className="grid grid-cols-2 gap-x-4 gap-y-1 text-sm">
            <dt className="text-muted-foreground">Tool</dt>
            <dd className="text-right font-mono">
              {status.tool ? (
                <>
                  {toolName(status.tool.number)}
                  {status.tool.target != null && <> → {toolName(status.tool.target)}</>}
                </>
              ) : (
                "—"
              )}
            </dd>
            <dt className="text-muted-foreground">Feed</dt>
            <dd className="text-right font-mono">
              {fmt(t?.feed, 0)} / {fmt(status.feedRequested ?? undefined, 0)}
            </dd>
            <dt className="text-muted-foreground">Spindle</dt>
            <dd className="text-right font-mono">
              {fmt(t?.spindleRpm, 0)} rpm
              {t?.spindleTemp != null && <> · {t.spindleTemp.toFixed(0)}°C</>}
            </dd>
            {status.job && (
              <>
                <dt className="text-muted-foreground">Job</dt>
                <dd className="text-right font-mono">
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
