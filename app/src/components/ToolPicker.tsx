import { Ruler } from "lucide-react";
import { useState } from "react";

import {
  AlertDialog,
  AlertDialogCancel,
  AlertDialogContent,
  AlertDialogDescription,
  AlertDialogFooter,
  AlertDialogHeader,
  AlertDialogTitle,
} from "@/components/ui/alert-dialog";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { act } from "@/lib/actions";
import { DEPTH_PROBE_TOOL, isProbe, LASER_TOOL, PROBE_TOOL, theTool, toolName } from "@/lib/machine";
import { useProgram } from "@/lib/program";
import { useStore } from "@/lib/store";

/** A stable empty list: a selector returning a fresh `[]` would re-render forever. */
const NO_TOOLS: number[] = [];

const capitalise = (s: string) => s.charAt(0).toUpperCase() + s.slice(1);

/** Picks the tool for a manual tool change: a number, one the program uses, or a probe. */
export function ToolPicker({ open, onOpenChange }: { open: boolean; onOpenChange: (open: boolean) => void }) {
  const current = useStore((s) => s.state?.status?.tool?.number ?? null);
  const programTools = useProgram((s) => s.analysis?.tools ?? NO_TOOLS);
  const hasLaser = useStore((s) => s.state?.status?.laser != null);
  const [draft, setDraft] = useState("");

  const change = async (tool: number) => {
    onOpenChange(false);
    setDraft("");
    await act({ type: "changeTool", tool });
  };
  const typed = Number(draft);
  /** Tools with a name rather than just a number. */
  const named = (t: number) => isProbe(t) || t === LASER_TOOL;
  const valid =
    draft.trim() !== "" &&
    Number.isInteger(typed) &&
    ((typed >= 0 && typed <= 999) || typed === PROBE_TOOL || (hasLaser && typed === LASER_TOOL));
  // The program's tools, then the probes and the laser, all just tools.
  const tools = [
    ...programTools.filter((t) => !named(t)),
    DEPTH_PROBE_TOOL,
    PROBE_TOOL,
    ...(hasLaser ? [LASER_TOOL] : []),
  ];

  return (
    <AlertDialog open={open} onOpenChange={onOpenChange}>
      <AlertDialogContent>
        <AlertDialogHeader>
          <AlertDialogTitle>Change tool</AlertDialogTitle>
          <AlertDialogDescription>
            {current !== null && current >= 0 ? `${capitalise(theTool(current))} is in the spindle. ` : ""}
            The machine moves to the change position and waits while you swap the tool, then
            measures it.
          </AlertDialogDescription>
        </AlertDialogHeader>

        <form
          className="flex gap-2"
          onSubmit={(e) => {
            e.preventDefault();
            if (valid) void change(typed);
          }}
        >
          <Input
            autoFocus
            inputMode="numeric"
            placeholder="Tool number"
            className="font-mono"
            value={draft}
            onChange={(e) => setDraft(e.target.value)}
          />
          <Button type="submit" disabled={!valid}>
            Change
          </Button>
        </form>

        {valid && named(typed) && (
          <p className="-mt-2 text-xs text-muted-foreground">T{typed} is {theTool(typed)}.</p>
        )}

        <div className="flex flex-wrap gap-2">
          {tools.map((t) => (
            <Button
              key={t}
              variant="outline"
              size="sm"
              disabled={t === current}
              title={named(t) ? `T${t}` : "Used by the program in the editor"}
              onClick={() => change(t)}
            >
              {named(t) ? toolName(t) : <span className="font-mono">T{t}</span>}
            </Button>
          ))}
        </div>

        <AlertDialogFooter>
          {current !== null && current >= 0 && current !== LASER_TOOL && (
            <Button
              variant="outline"
              className="sm:mr-auto"
              title="Measures the tool that's in on the tool sensor again, without changing it: after taking it out and putting it back, say"
              onClick={() => {
                onOpenChange(false);
                void act({ type: "remeasureTool" });
              }}
            >
              <Ruler /> Remeasure {toolName(current)}
            </Button>
          )}
          <AlertDialogCancel>Cancel</AlertDialogCancel>
        </AlertDialogFooter>
      </AlertDialogContent>
    </AlertDialog>
  );
}
