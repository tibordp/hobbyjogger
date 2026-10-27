import {
  ArrowDown,
  ArrowLeft,
  ArrowRight,
  ArrowUp,
  ChevronsDown,
  ChevronsUp,
  Octagon,
  RotateCcw,
  RotateCw,
} from "lucide-react";
import { useEffect, useState } from "react";

import { Button } from "@/components/ui/button";
import { Card, CardContent, CardHeader, CardTitle } from "@/components/ui/card";
import { ToggleGroup, ToggleGroupItem } from "@/components/ui/toggle-group";
import { act } from "@/lib/actions";
import { canJog, useConnected, useJobRunning, useMachineState } from "@/lib/machine";
import { useUi } from "@/lib/ui";

const STEPS = [0.01, 0.1, 1, 10, 50];
/** A-axis steps in degrees, paired with the linear steps. */
const A_STEPS = [0.1, 1, 5, 15, 90];
/** Fraction of the axis max rate. */
const SPEEDS = [0.1, 0.25, 0.5, 1];

type Dir = { x?: number; y?: number; z?: number; a?: number };

export function JogPanel() {
  const connected = useConnected();
  const state = useMachineState();
  const job = useJobRunning();
  const enabled = connected && canJog(state, job);
  const [stepIdx, setStepIdx] = useState(2);
  const speed = useUi((s) => s.jogSpeed);
  const setSpeed = (jogSpeed: number) => useUi.setState({ jogSpeed });

  const step = STEPS[stepIdx];
  const aStep = A_STEPS[stepIdx];

  const jog = (d: Dir) => {
    if (!enabled) return;
    void act({
      type: "jog",
      x: d.x !== undefined ? d.x * step : undefined,
      y: d.y !== undefined ? d.y * step : undefined,
      z: d.z !== undefined ? d.z * step : undefined,
      a: d.a !== undefined ? d.a * aStep : undefined,
      speed,
    });
  };

  // Keyboard jogging: arrows for XY, Page Up/Down for Z, Escape to stop. Key repeat is ignored so
  // holding a key doesn't queue a pile of moves.
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      const el = e.target as HTMLElement;
      if (el.closest("input, textarea, select, [contenteditable]") || e.repeat) return;
      const map: Record<string, Dir> = {
        ArrowLeft: { x: -1 },
        ArrowRight: { x: 1 },
        ArrowUp: { y: 1 },
        ArrowDown: { y: -1 },
        PageUp: { z: 1 },
        PageDown: { z: -1 },
      };
      if (e.key === "Escape") {
        void act({ type: "stopJog" });
      } else if (map[e.key]) {
        e.preventDefault();
        jog(map[e.key]);
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  });

  const Btn = ({ d, children, label }: { d: Dir; children: React.ReactNode; label: string }) => (
    <Button
      variant="secondary"
      className="h-12"
      disabled={!enabled}
      title={label}
      onClick={() => jog(d)}
    >
      {children}
    </Button>
  );

  return (
    <Card size="sm">
      <CardHeader>
        <CardTitle>Jog</CardTitle>
      </CardHeader>
      <CardContent className="space-y-4">
        <div className="flex gap-4">
          <div className="grid flex-1 grid-cols-3 gap-2">
            <span />
            <Btn d={{ y: 1 }} label="Y+ (↑)">
              <ArrowUp /> Y+
            </Btn>
            <span />
            <Btn d={{ x: -1 }} label="X− (←)">
              <ArrowLeft /> X−
            </Btn>
            <span />
            <Btn d={{ x: 1 }} label="X+ (→)">
              X+ <ArrowRight />
            </Btn>
            <span />
            <Btn d={{ y: -1 }} label="Y− (↓)">
              <ArrowDown /> Y−
            </Btn>
            <span />
          </div>
          <div className="grid w-20 grid-rows-3 gap-2">
            <Btn d={{ z: 1 }} label="Z+ (Page Up)">
              <ChevronsUp /> Z+
            </Btn>
            <span />
            <Btn d={{ z: -1 }} label="Z− (Page Down)">
              <ChevronsDown /> Z−
            </Btn>
          </div>
        </div>
        <div className="grid grid-cols-2 gap-2">
          <Btn d={{ a: -1 }} label="A−">
            <RotateCcw /> A− {aStep}°
          </Btn>
          <Btn d={{ a: 1 }} label="A+">
            A+ {aStep}° <RotateCw />
          </Btn>
        </div>

        <div className="space-y-1.5">
          <div className="text-xs text-muted-foreground">Step (mm)</div>
          <ToggleGroup
            type="single"
            variant="outline"
            size="sm"
            spacing={0}
            className="w-full"
            value={String(stepIdx)}
            onValueChange={(v) => v && setStepIdx(Number(v))}
          >
            {STEPS.map((s, i) => (
              <ToggleGroupItem key={s} value={String(i)} className="flex-1 font-mono">
                {s}
              </ToggleGroupItem>
            ))}
          </ToggleGroup>
        </div>
        <div className="space-y-1.5">
          <div className="text-xs text-muted-foreground">Speed (of max rate)</div>
          <ToggleGroup
            type="single"
            variant="outline"
            size="sm"
            spacing={0}
            className="w-full"
            value={String(speed)}
            onValueChange={(v) => v && setSpeed(Number(v))}
          >
            {SPEEDS.map((s) => (
              <ToggleGroupItem key={s} value={String(s)} className="flex-1 font-mono">
                {s * 100}%
              </ToggleGroupItem>
            ))}
          </ToggleGroup>
        </div>

        <Button
          variant="outline"
          className="w-full"
          disabled={!connected || job || state !== "run"}
          onClick={() => act({ type: "stopJog" })}
          title="Halts the machine and unlocks it again (Esc)"
        >
          <Octagon /> Stop jog
        </Button>
      </CardContent>
    </Card>
  );
}
