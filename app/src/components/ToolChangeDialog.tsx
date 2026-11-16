import { Check, LoaderCircle, OctagonX } from "lucide-react";

import {
  AlertDialog,
  AlertDialogContent,
  AlertDialogDescription,
  AlertDialogFooter,
  AlertDialogHeader,
  AlertDialogTitle,
} from "@/components/ui/alert-dialog";
import { Button } from "@/components/ui/button";
import { act } from "@/lib/actions";
import { DEPTH_PROBE_TOOL, isProbe, LASER_TOOL, theTool } from "@/lib/machine";
import { useStore } from "@/lib/store";

/**
 * Walks through a manual tool change, however it was started (button, program, console): moving
 * to the change position, swapping the tool, measuring it. It can't be dismissed while the change
 * is on; HALT stops it.
 */
export function ToolChangeDialog() {
  const tc = useStore((s) => s.state?.toolChange ?? null);
  const current = useStore((s) => s.state?.status?.tool?.number ?? null);
  const inJob = useStore((s) => s.state?.status?.job != null);
  if (!tc) return null;

  const probe = isProbe(tc.target);
  const the = theTool(tc.target);

  return (
    <AlertDialog open>
      <AlertDialogContent>
        <AlertDialogHeader>
          <AlertDialogTitle>
            {tc.remeasure ? `Remeasuring ${the}` : tc.phase === "waiting" ? `Insert ${the}` : `Changing to ${the}`}
          </AlertDialogTitle>
          <AlertDialogDescription>
            {inJob && "The job is waiting for a tool change. "}
            {tc.phase === "moving" && "Moving to the tool change position…"}
            {tc.phase === "waiting" && (
              <>
                {current !== null && current >= 0 && `Take out ${theTool(current)}, then `}
                {current !== null && current >= 0 ? "put" : "Put"} in {the}
                {tc.target === DEPTH_PROBE_TOOL ? "." : tc.target === LASER_TOOL ? " and plug in its cable." : " and tighten it."} When you confirm, the machine measures it on the tool sensor.
              </>
            )}
            {tc.phase === "measuring" && `Measuring ${the} on the tool sensor…`}
          </AlertDialogDescription>
        </AlertDialogHeader>

        {tc.phase !== "waiting" && (
          <div className="flex items-center gap-2 text-sm text-muted-foreground">
            <LoaderCircle className="size-4 animate-spin" />
            {tc.phase === "moving" ? "Moving" : "Measuring"}
          </div>
        )}

        <AlertDialogFooter>
          <Button
            variant="ghost"
            className="text-red-400 hover:text-red-300"
            onClick={() => act({ type: "halt" })}
            title="Stops the machine at once. Clear with Unlock."
          >
            <OctagonX /> Halt
          </Button>
          {tc.phase === "waiting" && (
            <Button autoFocus onClick={() => act({ type: "confirmToolChange" })}>
              <Check /> {probe ? "Probe" : "Tool"} is in, measure it
            </Button>
          )}
        </AlertDialogFooter>
      </AlertDialogContent>
    </AlertDialog>
  );
}
