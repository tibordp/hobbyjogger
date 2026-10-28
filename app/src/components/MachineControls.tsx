import { Crosshair, Fan, House, Lightbulb, LockOpen, Plug, Shrink, Wind, type LucideIcon } from "lucide-react";

import type { Output } from "@/bindings/Output";
import type { Outputs } from "@/bindings/Outputs";

import { Button } from "@/components/ui/button";
import { Toggle } from "@/components/ui/toggle";
import { Card, CardContent, CardHeader, CardTitle } from "@/components/ui/card";
import { act } from "@/lib/actions";
import { useConnected, useJobRunning, useMachineState } from "@/lib/machine";
import { useStore } from "@/lib/store";

const ZEROS: { label: string; axes: ("x" | "y" | "z" | "a")[] }[] = [
  { label: "X", axes: ["x"] },
  { label: "Y", axes: ["y"] },
  { label: "Z", axes: ["z"] },
  { label: "A", axes: ["a"] },
  { label: "XY", axes: ["x", "y"] },
];

const OUTPUTS: { key: keyof Outputs; output: Output; label: string; icon: LucideIcon; codes: string }[] = [
  { key: "light", output: "light", label: "Light", icon: Lightbulb, codes: "M821 / M822" },
  { key: "powerFan", output: "powerFan", label: "Power fan", icon: Fan, codes: "M801 / M802" },
  { key: "spindleFan", output: "spindleFan", label: "Spindle fan", icon: Fan, codes: "M811 / M812" },
  { key: "extOut", output: "extOut", label: "Ext. out", icon: Plug, codes: "M851 / M852" },
  { key: "air", output: "air", label: "Air", icon: Wind, codes: "M7 / M9" },
];

/** The outputs this machine reports, as toggles. PWM ones switch on at full power. */
function OutputToggles({ enabled }: { enabled: boolean }) {
  const outputs = useStore((s) => s.state?.outputs ?? null);
  if (!outputs) return null;
  const present = OUTPUTS.filter((o) => outputs[o.key] != null);
  if (present.length === 0) return null;
  return (
    <div className="flex flex-wrap gap-1.5">
      {present.map(({ key, output, label, icon: Icon, codes }) => {
        const v = outputs[key];
        const on = typeof v === "boolean" ? v : (v?.on ?? false);
        const power = typeof v === "object" && v?.on && v.power < 100 ? v.power : null;
        return (
          <Toggle
            key={key}
            size="sm"
            variant="outline"
            pressed={on}
            disabled={!enabled}
            title={codes}
            className="data-[state=on]:border-sky-500/60 data-[state=on]:bg-sky-500/15"
            onPressedChange={(p) => act({ type: "setOutput", output, on: p })}
          >
            <Icon /> {label}
            {power !== null && <span className="text-muted-foreground tabular-nums">{power}%</span>}
          </Toggle>
        );
      })}
    </div>
  );
}

export function MachineControls() {
  const connected = useConnected();
  const state = useMachineState();
  const job = useJobRunning();
  const changing = useStore((s) => s.state?.toolChange != null);
  const idle = state === "idle" && !job && !changing;

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
        <OutputToggles enabled={connected} />
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
