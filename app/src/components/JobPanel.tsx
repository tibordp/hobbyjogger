import { Pause, Play, Square } from "lucide-react";

import {
  AlertDialog,
  AlertDialogAction,
  AlertDialogCancel,
  AlertDialogContent,
  AlertDialogDescription,
  AlertDialogFooter,
  AlertDialogHeader,
  AlertDialogTitle,
  AlertDialogTrigger,
} from "@/components/ui/alert-dialog";
import { Overrides } from "@/components/Overrides";
import { Button } from "@/components/ui/button";
import { Card, CardContent, CardHeader, CardTitle } from "@/components/ui/card";
import { Progress } from "@/components/ui/progress";
import { act } from "@/lib/actions";
import { useConnected, useMachineState } from "@/lib/machine";
import { formatDuration } from "@/lib/program";
import { useStore } from "@/lib/store";

const fileName = (path: string) => path.split("/").pop() ?? path;

export function JobPanel() {
  const connected = useConnected();
  const state = useMachineState();
  const transfer = useStore((s) => s.state?.transfer ?? null);
  const job = useStore((s) => s.state?.job ?? null);
  const progress = useStore((s) => s.state?.status?.job ?? null);

  let body;
  if (transfer) {
    const pct = transfer.total > 0 ? (transfer.done / transfer.total) * 100 : 0;
    const verb = transfer.direction === "upload" ? "Uploading" : "Downloading";
    body = (
      <div className="space-y-2">
        <div className="flex justify-between text-sm">
          <span className="truncate">
            {verb} {fileName(transfer.path)}
          </span>
          <span className="tabular-nums text-muted-foreground">{Math.floor(pct)}%</span>
        </div>
        <Progress value={pct} />
        {transfer.run && <div className="text-xs text-muted-foreground">Starts when uploaded.</div>}
      </div>
    );
  } else if (progress) {
    const status =
      job?.aborting ? "Stopping after the queued moves…"
      : state === "wait" ? "Pausing after the queued moves…"
      : state === "pause" ? "Paused"
      : "Running";
    body = (
      <div className="space-y-2">
        <div className="flex justify-between gap-2 text-sm">
          <span className="truncate font-medium">{job ? fileName(job.path) : "Job started elsewhere"}</span>
          <span className="tabular-nums text-muted-foreground">{progress.percent}%</span>
        </div>
        <Progress value={progress.percent} />
        <div className="flex justify-between text-xs text-muted-foreground">
          <span>{status}</span>
          <span className="tabular-nums">
            line {progress.lines} · {formatDuration(progress.elapsedSecs)}
          </span>
        </div>
        <div className="grid grid-cols-2 gap-2 pt-1">
          {state === "pause" ? (
            <Button variant="secondary" onClick={() => act({ type: "resume" })}>
              <Play /> Resume
            </Button>
          ) : (
            <Button
              variant="secondary"
              disabled={state !== "run" || job?.aborting}
              onClick={() => act({ type: "pause" })}
              title="Pauses after the moves already queued (a few seconds). HALT stops at once."
            >
              <Pause /> Pause
            </Button>
          )}
          <AlertDialog>
            <AlertDialogTrigger asChild>
              <Button variant="outline" disabled={job?.aborting}>
                <Square /> Stop
              </Button>
            </AlertDialogTrigger>
            <AlertDialogContent>
              <AlertDialogHeader>
                <AlertDialogTitle>Stop the job?</AlertDialogTitle>
                <AlertDialogDescription>
                  The machine finishes the moves already queued (a few seconds), then stops the
                  spindle. Use HALT to stop at once.
                </AlertDialogDescription>
              </AlertDialogHeader>
              <AlertDialogFooter>
                <AlertDialogCancel>Keep running</AlertDialogCancel>
                <AlertDialogAction variant="destructive" onClick={() => act({ type: "abort" })}>
                  Stop job
                </AlertDialogAction>
              </AlertDialogFooter>
            </AlertDialogContent>
          </AlertDialog>
        </div>
      </div>
    );
  } else {
    body = null;
  }

  return (
    <Card size="sm">
      <CardHeader className="flex flex-row items-center gap-2">
        <CardTitle className="flex-1">Job</CardTitle>
        {!body && (
          <span className="text-xs text-muted-foreground">{connected ? "None running" : "Not connected"}</span>
        )}
      </CardHeader>
      {(body || connected) && (
        <CardContent className="space-y-3">
          {body}
          {/* Overrides apply to anything that moves, so they stay in view without a job too. */}
          {connected && <div className={body ? "border-t pt-3" : undefined}><Overrides /></div>}
        </CardContent>
      )}
    </Card>
  );
}
