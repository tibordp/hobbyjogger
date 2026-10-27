import { Crosshair, House, LockOpen, Shrink } from "lucide-react";

import { Button } from "@/components/ui/button";
import { Card, CardContent, CardHeader, CardTitle } from "@/components/ui/card";
import { act } from "@/lib/actions";
import { useConnected, useJobRunning, useMachineState } from "@/lib/machine";

const ZEROS: { label: string; axes: ("x" | "y" | "z" | "a")[] }[] = [
  { label: "X", axes: ["x"] },
  { label: "Y", axes: ["y"] },
  { label: "Z", axes: ["z"] },
  { label: "A", axes: ["a"] },
  { label: "XY", axes: ["x", "y"] },
];

export function MachineControls() {
  const connected = useConnected();
  const state = useMachineState();
  const job = useJobRunning();
  const idle = state === "idle" && !job;

  return (
    <Card size="sm">
      <CardHeader>
        <CardTitle>Machine</CardTitle>
      </CardHeader>
      <CardContent className="space-y-3">
        <div className="grid grid-cols-2 gap-2">
          <Button
            variant="secondary"
            disabled={!connected || job || !(state === "idle" || state === "alarm")}
            onClick={() => act({ type: "home" })}
          >
            <House /> Home
          </Button>
          <Button variant="secondary" disabled={!connected} onClick={() => act({ type: "unlock" })}>
            <LockOpen /> Unlock
          </Button>
        </div>
        <div>
          <div className="mb-1.5 flex items-center gap-1.5 text-xs text-muted-foreground">
            <Crosshair className="size-3.5" /> Set work zero
            <span className="ml-auto">or click a position to type a value</span>
          </div>
          <div className="grid grid-cols-5 gap-2">
            {ZEROS.map((z) => (
              <Button
                key={z.label}
                variant="outline"
                size="sm"
                disabled={!idle}
                onClick={() =>
                  act({ type: "setWork", ...Object.fromEntries(z.axes.map((a) => [a, 0])) })
                }
              >
                {z.label}0
              </Button>
            ))}
          </div>
        </div>
        <Button
          variant="outline"
          size="sm"
          className="w-full"
          disabled={!idle}
          onClick={() => act({ type: "shrinkA" })}
          title="Drop whole turns from A so it reads within ±360° (G92.4 A0 S0). Doesn't move."
        >
          <Shrink /> Shrink A
        </Button>
      </CardContent>
    </Card>
  );
}
