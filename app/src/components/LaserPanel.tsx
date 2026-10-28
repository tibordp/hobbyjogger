import { Flame } from "lucide-react";
import { useEffect } from "react";

import { Badge } from "@/components/ui/badge";
import { Card, CardContent, CardHeader, CardTitle } from "@/components/ui/card";
import { Toggle } from "@/components/ui/toggle";
import { act } from "@/lib/actions";
import { useJobRunning, useMachineState } from "@/lib/machine";
import { useStore } from "@/lib/store";

/** Test fire, while the laser is the tool. Changing to and from it is a tool change. */
export function LaserPanel() {
  const laser = useStore((s) => s.state?.status?.laser ?? null);
  const state = useMachineState();
  const job = useJobRunning();
  const busy = useStore((s) => s.state?.toolChange != null || s.state?.probe?.running != null);
  const idle = state === "idle" && !job && !busy;
  const testing = laser?.testing ?? false;

  // Don't leave the beam on when the window goes away.
  useEffect(() => {
    if (!testing) return;
    const off = () => void act({ type: "laserTest", on: false });
    window.addEventListener("blur", off);
    return () => window.removeEventListener("blur", off);
  }, [testing]);

  if (!laser?.mode) return null;

  return (
    <Card size="sm">
      <CardHeader className="flex flex-row items-center justify-between">
        <CardTitle>Laser</CardTitle>
        {laser.on && <Badge className="bg-red-600 text-white uppercase">Beam on</Badge>}
      </CardHeader>
      <CardContent className="space-y-3">
        <Toggle
          variant="outline"
          pressed={testing}
          disabled={!testing && !idle}
          className="w-full data-[state=on]:border-red-500 data-[state=on]:bg-red-600 data-[state=on]:text-white"
          onPressedChange={(on) => act({ type: "laserTest", on })}
          title="Fires at the configured test power (laser_module_test_power) until you turn it off"
        >
          <Flame /> {testing ? "Firing" : "Test fire"}
        </Toggle>
        <p className="text-xs text-amber-500">Wear laser goggles while it can fire.</p>
      </CardContent>
    </Card>
  );
}
