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
import { type Dir, dirKey, press, release, releaseAll, useJogPresses } from "@/lib/jog";
import { canJog, useConnected, useJobRunning, useMachineState } from "@/lib/machine";
import { load, save } from "@/lib/persist";
import { useUi } from "@/lib/ui";
import { cn } from "@/lib/utils";

const STEPS = [0.01, 0.1, 1, 10, 50];
/** A-axis steps in degrees, paired with the linear steps. */
const A_STEPS = [0.1, 1, 5, 15, 90];
/** Fraction of the axis max rate. */
const SPEEDS = [0.1, 0.25, 0.5, 1];

const KEYS: Record<string, Dir> = {
  ArrowLeft: { x: -1 },
  ArrowRight: { x: 1 },
  ArrowUp: { y: 1 },
  ArrowDown: { y: -1 },
  PageUp: { z: 1 },
  PageDown: { z: -1 },
};

/** A jog button: click for a step, hold for continuous motion. */
function JogButton({
  dir,
  step,
  enabled,
  label,
  children,
}: {
  dir: Dir;
  step: () => void;
  enabled: boolean;
  label: string;
  children: React.ReactNode;
}) {
  // Lit while pressed, by pointer or key; brighter once it's moving continuously.
  const pressed = useJogPresses((s) => s.pressed.includes(dirKey(dir)));
  const held = useJogPresses((s) => s.held.includes(dirKey(dir)));
  return (
    <Button
      variant="secondary"
      className={cn(
        "h-12 touch-none",
        pressed && "ring-2 ring-sky-400/60",
        held && "bg-sky-500/25 ring-sky-400",
      )}
      disabled={!enabled}
      title={`${label}: click for a step, hold to keep moving`}
      onPointerDown={(e) => {
        if (e.button !== 0) return;
        press("pointer", dir, step);
        // Released wherever the pointer goes up, even if the button got disabled meanwhile.
        const up = () => {
          window.removeEventListener("pointerup", up);
          window.removeEventListener("pointercancel", up);
          release("pointer");
        };
        window.addEventListener("pointerup", up);
        window.addEventListener("pointercancel", up);
      }}
      // Keyboard activation (Enter, Space) is a step.
      onClick={(e) => e.detail === 0 && step()}
    >
      {children}
    </Button>
  );
}

export function JogPanel() {
  const connected = useConnected();
  const state = useMachineState();
  const job = useJobRunning();
  const enabled = connected && canJog(state, job);
  const [stepIdx, setStepIdx] = useState(() => {
    const i = load("jogStep", 2);
    return Number.isInteger(i) && i >= 0 && i < STEPS.length ? i : 2;
  });
  useEffect(() => save("jogStep", stepIdx), [stepIdx]);
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

  // Keyboard jogging: arrows for XY, Page Up/Down for Z; tap for a step, hold to keep moving
  // (two arrows for a diagonal). Escape halts. Key repeat is ignored: holding is handled above it.
  useEffect(() => {
    const onDown = (e: KeyboardEvent) => {
      const el = e.target as HTMLElement;
      if (el.closest("input, textarea, select, [contenteditable]")) return;
      if (e.key === "Escape") {
        releaseAll();
        void act({ type: "stopJog" });
      } else if (KEYS[e.key] && !e.metaKey && !e.ctrlKey && !e.altKey) {
        e.preventDefault();
        if (!e.repeat && enabled) press(e.key, KEYS[e.key], () => jog(KEYS[e.key]));
      }
    };
    const onUp = (e: KeyboardEvent) => release(e.key);
    window.addEventListener("keydown", onDown);
    window.addEventListener("keyup", onUp);
    return () => {
      window.removeEventListener("keydown", onDown);
      window.removeEventListener("keyup", onUp);
    };
  });
  // Key-ups don't arrive while the window is in the background.
  useEffect(() => {
    window.addEventListener("blur", releaseAll);
    return () => window.removeEventListener("blur", releaseAll);
  }, []);


  return (
    <Card size="sm">
      <CardHeader>
        <CardTitle>Jog</CardTitle>
      </CardHeader>
      <CardContent className="space-y-4">
        <div className="flex gap-4">
          <div className="grid flex-1 grid-cols-3 gap-2">
            <span />
            <JogButton dir={{ y: 1 }} step={() => jog({ y: 1 })} enabled={enabled} label="Y+ (↑)">
              <ArrowUp /> Y+
            </JogButton>
            <span />
            <JogButton dir={{ x: -1 }} step={() => jog({ x: -1 })} enabled={enabled} label="X− (←)">
              <ArrowLeft /> X−
            </JogButton>
            <span />
            <JogButton dir={{ x: 1 }} step={() => jog({ x: 1 })} enabled={enabled} label="X+ (→)">
              X+ <ArrowRight />
            </JogButton>
            <span />
            <JogButton dir={{ y: -1 }} step={() => jog({ y: -1 })} enabled={enabled} label="Y− (↓)">
              <ArrowDown /> Y−
            </JogButton>
            <span />
          </div>
          <div className="grid w-20 grid-rows-3 gap-2">
            <JogButton dir={{ z: 1 }} step={() => jog({ z: 1 })} enabled={enabled} label="Z+ (Page Up)">
              <ChevronsUp /> Z+
            </JogButton>
            <span />
            <JogButton dir={{ z: -1 }} step={() => jog({ z: -1 })} enabled={enabled} label="Z− (Page Down)">
              <ChevronsDown /> Z−
            </JogButton>
          </div>
        </div>
        <div className="grid grid-cols-2 gap-2">
          <JogButton dir={{ a: -1 }} step={() => jog({ a: -1 })} enabled={enabled} label="A−">
            <RotateCcw /> A− {aStep}°
          </JogButton>
          <JogButton dir={{ a: 1 }} step={() => jog({ a: 1 })} enabled={enabled} label="A+">
            A+ {aStep}° <RotateCw />
          </JogButton>
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
