import { Crosshair, Move } from "lucide-react";
import { useEffect, useRef, useState } from "react";

import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuLabel,
  DropdownMenuSeparator,
  DropdownMenuShortcut,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import { act } from "@/lib/actions";
import { canJog, useConnected, useJobRunning, useMachineState } from "@/lib/machine";
import { useProgram } from "@/lib/program";
import { useStore } from "@/lib/store";
import { useUi } from "@/lib/ui";
import { cn } from "@/lib/utils";

/** Used until the machine reports its Z limit (the Z1's). */
const DEFAULT_MIN_Z = -102;
const W = 44;
/** The track's left and right edges, px. */
const L = 18;
const R = 28;

/**
 * Z beside the bed, `height` px tall with the track `pad` px in from either end (to line up with
 * the bed drawing's edges): travel, tool, work zero and the program's depth range, in machine
 * coordinates. Clicking a height offers to move Z there or to put the work Z zero there, as the
 * bed does for X/Y. `onHover` reports the machine Z under the pointer.
 */
export function ZBar({ height, pad, onHover }: { height: number; pad: number; onHover: (z: number | null) => void }) {
  const connected = useConnected();
  const t = useStore((s) => s.telemetry);
  const status = useStore((s) => s.state?.status ?? null);
  const state = useMachineState();
  const job = useJobRunning();
  const travel = useStore((s) => s.state?.machine?.travel);
  const probe = useStore((s) => s.state?.probe ?? null);
  const analysis = useProgram((s) => s.analysis);
  const speed = useUi((s) => s.jogSpeed);
  const host = useRef<HTMLDivElement>(null);
  const [hover, setHover] = useState<number | null>(null);
  const [menu, setMenu] = useState<{ z: number; top: number } | null>(null);
  const [target, setTarget] = useState<number | null>(null);

  const minZ = travel?.min[2] ?? DEFAULT_MIN_Z;
  const maxZ = travel?.max[2] ?? -1;
  const span = Math.max(height - 2 * pad, 1);
  const py = (z: number) => pad + ((maxZ - z) / (maxZ - minZ)) * span;
  const toZ = (y: number) => maxZ - ((y - pad) / span) * (maxZ - minZ);

  // Machine − work along Z, tool length included: what the DRO's work Z is measured from.
  const offset = connected && t ? t.machine[2] - t.work[2] : status?.workOffset[2];
  const canAct = connected && state === "idle" && !job;

  // Clear the target marker once the move is over (as on the bed).
  const sawRun = useRef(false);
  useEffect(() => {
    if (target === null) {
      sawRun.current = false;
      return;
    }
    if (state === "run") sawRun.current = true;
    if ((state === "idle" && sawRun.current) || !canJog(state, job)) setTarget(null);
  }, [target, state, job]);

  const zAt = (e: React.MouseEvent): number | null => {
    const box = host.current?.getBoundingClientRect();
    if (!box) return null;
    const z = toZ(e.clientY - box.top);
    return z < minZ || z > maxZ ? null : z;
  };
  const hoverAt = (z: number | null) => {
    setHover(z);
    onHover(z);
  };
  const openMenu = (e: React.MouseEvent) => {
    const z = zAt(e);
    if (z === null || !connected || height === 0) return;
    e.preventDefault();
    setMenu({ z: Math.round(z * 100) / 100, top: py(z) });
  };

  const ticks = [];
  for (let z = Math.ceil(minZ / 10) * 10; z <= maxZ; z += 10) {
    const major = z % 50 === 0;
    ticks.push(
      <line key={z} x1={major ? L - 7 : L - 4} y1={py(z)} x2={L} y2={py(z)} className="stroke-border" strokeWidth={1} />,
    );
  }

  const band = (lo: number, hi: number) => ({ y: py(hi), height: Math.max(py(lo) - py(hi), 1.5) });
  const tool = connected && t ? py(t.machine[2]) : null;
  const zero = connected && offset !== undefined ? py(offset) : null;
  const cut = analysis?.cutBounds && offset !== undefined ? band(analysis.cutBounds.min[2] + offset, analysis.cutBounds.max[2] + offset) : null;
  const all = analysis?.bounds && offset !== undefined ? band(analysis.bounds.min[2] + offset, analysis.bounds.max[2] + offset) : null;
  const touched = probe?.result?.z != null && !probe.running ? py(probe.result.z) : null;

  return (
    <div ref={host} className="relative shrink-0" style={{ width: W, height }}>
      <DropdownMenu open={menu !== null} onOpenChange={(open) => !open && setMenu(null)}>
        <DropdownMenuTrigger asChild>
          <span className="pointer-events-none absolute size-0" style={{ left: R, top: menu?.top ?? 0 }} />
        </DropdownMenuTrigger>
        {menu && (
          <DropdownMenuContent align="start" side="left" sideOffset={14} className="min-w-56">
            <DropdownMenuLabel className="font-mono text-xs font-normal">
              {offset !== undefined && <div>W Z{(menu.z - offset).toFixed(2)}</div>}
              <div className="text-muted-foreground">M Z{menu.z.toFixed(2)}</div>
            </DropdownMenuLabel>
            <DropdownMenuSeparator />
            <DropdownMenuItem
              disabled={!canAct}
              onSelect={() => {
                setTarget(menu.z);
                void act({ type: "moveTo", z: menu.z, speed }).then((ok) => ok || setTarget(null));
              }}
            >
              <Move /> Move Z here
              <DropdownMenuShortcut>{Math.round(speed * 100)}%</DropdownMenuShortcut>
            </DropdownMenuItem>
            <DropdownMenuItem disabled={!canAct} onSelect={() => void act({ type: "setOrigin", z: menu.z })}>
              <Crosshair /> Set work Z zero here
              <DropdownMenuShortcut>Z</DropdownMenuShortcut>
            </DropdownMenuItem>
          </DropdownMenuContent>
        )}
      </DropdownMenu>
      {height > 0 && (
        <svg
          width={W}
          height={height}
          className={cn("block", connected && "cursor-crosshair")}
          onMouseMove={(e) => hoverAt(zAt(e))}
          onMouseLeave={() => hoverAt(null)}
          onClick={openMenu}
          onContextMenu={openMenu}
        >
          <title>Z travel: tool, work zero and the program's depth range</title>
          <rect x={L} y={pad} width={R - L} height={span} className="fill-muted/30 stroke-border" strokeWidth={1} />
          {ticks}
          {all && <rect x={L + 1} width={R - L - 2} {...all} className="pointer-events-none fill-muted-foreground/25" />}
          {cut && <rect x={L + 1} width={R - L - 2} {...cut} className="pointer-events-none fill-fuchsia-400/70" />}
          {zero !== null && (
            <line x1={L - 6} y1={zero} x2={R + 6} y2={zero} className="pointer-events-none stroke-emerald-500" strokeWidth={1.5} />
          )}
          {touched !== null && (
            <line x1={L} y1={touched} x2={R} y2={touched} className="pointer-events-none stroke-sky-300" strokeWidth={1} strokeDasharray="2 1.5" />
          )}
          {hover !== null && connected && !menu && (
            <line x1={0} y1={py(hover)} x2={W} y2={py(hover)} className="pointer-events-none stroke-sky-400/50" strokeWidth={1} />
          )}
          {menu && <line x1={L - 4} y1={menu.top} x2={R + 4} y2={menu.top} className="pointer-events-none stroke-sky-400" strokeWidth={1.5} />}
          {target !== null && (
            <g className="pointer-events-none stroke-amber-400" strokeWidth={1.5}>
              {tool !== null && <line x1={R + 4} y1={tool} x2={R + 4} y2={py(target)} strokeDasharray="2 1.5" />}
              <line x1={L - 4} y1={py(target)} x2={R + 8} y2={py(target)} />
            </g>
          )}
          {tool !== null && (
            <g className="pointer-events-none">
              <line x1={L - 2} y1={tool} x2={R + 2} y2={tool} className="stroke-sky-400" strokeWidth={1.5} />
              <path d={`M${R + 2} ${tool} l7 -4.5 v9 z`} className="fill-sky-400" />
            </g>
          )}
        </svg>
      )}
    </div>
  );
}
