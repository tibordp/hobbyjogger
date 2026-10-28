import { TriangleAlert } from "lucide-react";

import {
  AlertDialog,
  AlertDialogAction,
  AlertDialogCancel,
  AlertDialogContent,
  AlertDialogDescription,
  AlertDialogFooter,
  AlertDialogHeader,
  AlertDialogTitle,
} from "@/components/ui/alert-dialog";
import { act } from "@/lib/actions";
import { toolName, useStatus } from "@/lib/machine";
import { currentText, fitProblems, formatDuration, useProgram } from "@/lib/program";
import { useStore } from "@/lib/store";


/**
 * Confirms running the program in the editor. Jobs only start from here, so the editor always
 * shows what runs. An unchanged copy of a file on the machine runs from there; anything else is
 * uploaded first.
 */
export function RunDialog({
  open,
  onOpenChange,
}: {
  open: boolean;
  onOpenChange: (open: boolean) => void;
}) {
  const status = useStatus();
  const travel = useStore((s) => s.state?.machine?.travel);
  const name = useProgram((s) => s.name);
  const a = useProgram((s) => s.analysis);
  const version = useProgram((s) => s.version);
  const onMachine = useProgram((s) => s.onMachine);
  const remote = onMachine?.version === version ? onMachine.path : null;
  const target = remote ?? `/sd/gcodes/${name.trim()}`;

  const problems = a && status && travel ? fitProblems(a, status.workOffset, travel) : [];
  const errors = a?.diagnostics.filter((d) => d.severity === "error").length ?? 0;
  const offset = status?.workOffset;
  const b = a?.bounds;
  const range = (i: number) => b && `${b.min[i].toFixed(2)} … ${b.max[i].toFixed(2)}`;

  const run = async () => {
    if (remote) {
      await act({ type: "play", path: remote });
    } else if (await act({ type: "upload", name: name.trim(), text: currentText(), run: true })) {
      useProgram.setState({ onMachine: { path: target, version } });
    }
  };

  return (
    <AlertDialog open={open} onOpenChange={onOpenChange}>
      <AlertDialogContent>
        <AlertDialogHeader>
          <AlertDialogTitle>Run {name}?</AlertDialogTitle>
          <AlertDialogDescription>
            {remote ? (
              <>Runs <span className="font-mono">{remote}</span> from the machine's SD card.</>
            ) : (
              <>Uploads the program as <span className="font-mono">{target}</span>, then starts it.</>
            )}{" "}
            The spindle starts if the program says so; keep a hand near HALT.
          </AlertDialogDescription>
        </AlertDialogHeader>

        <dl className="grid grid-cols-[auto_1fr] gap-x-4 gap-y-1 text-sm">
          <dt className="text-muted-foreground">Tool in spindle</dt>
          <dd className="font-mono">{status?.tool ? toolName(status.tool.number) : "—"}</dd>
          {a && a.tools.length > 0 && (
            <>
              <dt className="text-muted-foreground">Tool changes</dt>
              <dd className="font-mono">{a.tools.map(toolName).join(", ")}</dd>
            </>
          )}
          {offset && (
            <>
              <dt className="text-muted-foreground">Work zero (machine)</dt>
              <dd className="font-mono whitespace-nowrap">
                X{offset[0].toFixed(2)} Y{offset[1].toFixed(2)} Z{offset[2].toFixed(2)}
              </dd>
            </>
          )}
          {b && (
            <>
              <dt className="text-muted-foreground">Extent (work)</dt>
              <dd className="font-mono">
                X {range(0)}
                <br />Y {range(1)}
                <br />Z {range(2)}
              </dd>
            </>
          )}
          {a && (
            <>
              <dt className="text-muted-foreground">Estimated time</dt>
              <dd>≈{formatDuration(a.estimatedSecs)}</dd>
            </>
          )}
        </dl>

        {(problems.length > 0 || errors > 0 || a?.usesA) && (
          <div className="space-y-1 rounded-md border border-amber-500/50 bg-amber-500/10 p-2 text-sm">
            {problems.map((p) => (
              <div key={p.short} className="flex gap-2">
                <TriangleAlert className="mt-0.5 size-4 shrink-0 text-amber-500" />
                <span>{p.detail}. The machine will halt there.</span>
              </div>
            ))}
            {errors > 0 && (
              <div className="flex gap-2">
                <TriangleAlert className="mt-0.5 size-4 shrink-0 text-amber-500" />
                <span>
                  The program has {errors} error{errors > 1 && "s"} (marked in the editor).
                </span>
              </div>
            )}
            {a?.usesA && (
              <div className="flex gap-2">
                <TriangleAlert className="mt-0.5 size-4 shrink-0 text-amber-500" />
                <span>The program turns the A axis, which the preview and checks leave out.</span>
              </div>
            )}
          </div>
        )}

        <AlertDialogFooter>
          <AlertDialogCancel>Cancel</AlertDialogCancel>
          <AlertDialogAction onClick={() => void run()}>Run</AlertDialogAction>
        </AlertDialogFooter>
      </AlertDialogContent>
    </AlertDialog>
  );
}
