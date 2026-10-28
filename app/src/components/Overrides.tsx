import { Minus, Plus } from "lucide-react";

import type { Request } from "@/bindings/Request";
import { Button } from "@/components/ui/button";
import { act } from "@/lib/actions";
import { useConnected } from "@/lib/machine";
import { useStore } from "@/lib/store";
import { cn } from "@/lib/utils";

type Which = "feed" | "spindle" | "laser";

/** Ranges as the core accepts them (M220, M223, M325). */
const RANGE: Record<Which, [number, number]> = {
  feed: [10, 300],
  spindle: [50, 200],
  laser: [10, 200],
};
const STEP = 10;

function Stepper({ which, label, value, enabled }: { which: Which; label: string; value: number | null; enabled: boolean }) {
  const [min, max] = RANGE[which];
  const set = (v: number) => act({ type: "setOverride", [which]: Math.min(max, Math.max(min, v)) } as Request);
  const pct = value === null ? null : Math.round(value);
  return (
    <>
      <span className="text-muted-foreground">{label}</span>
      <div className="flex items-center justify-end gap-1">
        <Button size="icon-xs" variant="outline" disabled={!enabled || pct === null || pct <= min} onClick={() => set((pct ?? 100) - STEP)}>
          <Minus />
        </Button>
        <button
          type="button"
          disabled={!enabled || pct === 100}
          title={enabled && pct !== 100 ? "Back to 100 %" : undefined}
          className={cn(
            "w-12 rounded-md text-center font-medium tabular-nums enabled:cursor-pointer enabled:hover:bg-muted",
            // Easy to forget: anything but 100 % stands out.
            pct !== null && pct !== 100 && "bg-amber-500/15 text-amber-400",
          )}
          onClick={() => set(100)}
        >
          {pct === null ? "—" : `${pct}%`}
        </button>
        <Button size="icon-xs" variant="outline" disabled={!enabled || pct === null || pct >= max} onClick={() => set((pct ?? 100) + STEP)}>
          <Plus />
        </Button>
      </div>
    </>
  );
}

/** Feed and spindle (or, in laser mode, laser power) overrides. They work during a job too. */
export function Overrides() {
  const connected = useConnected();
  const feed = useStore((s) => s.state?.status?.feedOverridePct ?? null);
  const spindle = useStore((s) => s.state?.status?.spindleOverridePct ?? null);
  const laser = useStore((s) => s.state?.status?.laser ?? null);

  return (
    <div className="grid grid-cols-[auto_1fr] items-center gap-x-3 gap-y-1.5 text-sm">
      <Stepper which="feed" label="Feed override" value={feed} enabled={connected} />
      {laser?.mode ? (
        <Stepper which="laser" label="Laser power" value={laser.scale} enabled={connected} />
      ) : (
        <Stepper which="spindle" label="Spindle override" value={spindle} enabled={connected} />
      )}
    </div>
  );
}
