import { MousePointerClick } from "lucide-react";
import { useEffect, useRef, useState } from "react";

import { Toggle } from "@/components/ui/toggle";
import { act } from "@/lib/actions";
import { canJog, useConnected, useJobRunning, useMachineState, useStatus } from "@/lib/machine";
import { useStore } from "@/lib/store";
import { useUi } from "@/lib/ui";
import { cn } from "@/lib/utils";

/** Used until the machine reports its limits (Z1 values). */
const DEFAULT_MIN = [-207, -206];
const MARGIN = 8;

type Point = { x: number; y: number };

/**
 * Top-down view of the work area in machine coordinates: tool position and work zero. Clicking
 * moves the head there (X/Y only, at the current height) when click-to-move is on. Placeholder
 * for the 3D view.
 */
export function BedView() {
  const connected = useConnected();
  const t = useStore((s) => s.telemetry);
  const status = useStatus();
  const state = useMachineState();
  const job = useJobRunning();
  const travel = useStore((s) => s.state?.machine?.travel);
  const clickToMove = useUi((s) => s.clickToMove);
  const speed = useUi((s) => s.jogSpeed);
  const svg = useRef<SVGSVGElement>(null);
  const [hover, setHover] = useState<Point | null>(null);
  const [target, setTarget] = useState<Point | null>(null);

  const minX = travel?.min[0] ?? DEFAULT_MIN[0];
  const minY = travel?.min[1] ?? DEFAULT_MIN[1];
  const maxX = travel?.max[0] ?? -1;
  const maxY = travel?.max[1] ?? -1;
  const w = maxX - minX;
  const h = maxY - minY;

  // SVG y grows downwards; machine +Y is towards the back, drawn at the top.
  const sx = (x: number) => x - minX;
  const sy = (y: number) => maxY - y;

  const canMove = connected && clickToMove && state === "idle" && !job;

  // Clear the target marker once the move is over: back to idle after running (the target may
  // have been clamped to the travel limits, so it isn't always reached), or stopped otherwise.
  const sawRun = useRef(false);
  useEffect(() => {
    if (!target) {
      sawRun.current = false;
      return;
    }
    if (state === "run") sawRun.current = true;
    if ((state === "idle" && sawRun.current) || !canJog(state, job)) setTarget(null);
  }, [target, state, job]);

  const toMachine = (e: React.MouseEvent): Point | null => {
    const el = svg.current;
    const ctm = el?.getScreenCTM();
    if (!el || !ctm) return null;
    const p = new DOMPoint(e.clientX, e.clientY).matrixTransform(ctm.inverse());
    const x = p.x + minX;
    const y = maxY - p.y;
    if (x < minX || x > maxX || y < minY || y > maxY) return null;
    return { x, y };
  };

  const grid = [];
  for (let v = 0; v <= w; v += 10) {
    const major = Math.round(v) % 50 === 0;
    grid.push(
      <line key={`x${v}`} x1={v} y1={0} x2={v} y2={h} className={major ? "stroke-border" : "stroke-border/40"} strokeWidth={major ? 0.4 : 0.2} />,
    );
  }
  for (let v = 0; v <= h; v += 10) {
    const major = Math.round(v) % 50 === 0;
    grid.push(
      <line key={`y${v}`} x1={0} y1={v} x2={w} y2={v} className={major ? "stroke-border" : "stroke-border/40"} strokeWidth={major ? 0.4 : 0.2} />,
    );
  }

  const tool = connected && t ? { x: sx(t.machine[0]), y: sy(t.machine[1]) } : null;
  const zero = connected && status ? { x: sx(status.workOffset[0]), y: sy(status.workOffset[1]) } : null;
  const offset = status?.workOffset;

  return (
    <div className="flex h-full flex-col">
      <div className="flex h-11 shrink-0 items-center gap-3 border-b px-3">
        <div className="font-mono text-xs">
          {hover && (
            <>
              {offset && (
                <span>
                  W X{(hover.x - offset[0]).toFixed(2)} Y{(hover.y - offset[1]).toFixed(2)}
                </span>
              )}
              <span className="ml-3 text-muted-foreground">
                M X{hover.x.toFixed(2)} Y{hover.y.toFixed(2)}
              </span>
            </>
          )}
        </div>
        <div className="flex-1" />
        {clickToMove && (
          <span className="text-xs text-muted-foreground">
            Moves X/Y at the current height, {Math.round(speed * 100)}% speed
          </span>
        )}
        <Toggle
          size="sm"
          variant="outline"
          pressed={clickToMove}
          onPressedChange={(clickToMove) => useUi.setState({ clickToMove })}
          title="When on, clicking the bed moves the head there (X/Y only, at the current Z)"
        >
          <MousePointerClick /> Click to move
        </Toggle>
      </div>

      <div className="flex min-h-0 flex-1 items-center justify-center p-4">
        <svg
          ref={svg}
          viewBox={`${-MARGIN} ${-MARGIN} ${w + 2 * MARGIN} ${h + 2 * MARGIN}`}
          className={cn("h-full max-h-full w-full max-w-full", canMove && "cursor-crosshair")}
          onMouseMove={(e) => setHover(toMachine(e))}
          onMouseLeave={() => setHover(null)}
          onClick={(e) => {
            const p = toMachine(e);
            if (!p || !canMove) return;
            setTarget(p);
            void act({ type: "moveTo", x: p.x, y: p.y, speed }).then((ok) => ok || setTarget(null));
          }}
        >
          <rect x={0} y={0} width={w} height={h} className="fill-muted/30 stroke-border" strokeWidth={0.6} />
          {grid}
          {zero && (
            <g className="stroke-emerald-500" strokeWidth={0.6}>
              <line x1={zero.x - 6} y1={zero.y} x2={zero.x + 6} y2={zero.y} />
              <line x1={zero.x} y1={zero.y - 6} x2={zero.x} y2={zero.y + 6} />
              <circle cx={zero.x} cy={zero.y} r={2.5} fill="none" />
            </g>
          )}
          {hover && canMove && (
            <g className="pointer-events-none stroke-sky-400/50" strokeWidth={0.3}>
              <line x1={sx(hover.x)} y1={0} x2={sx(hover.x)} y2={h} />
              <line x1={0} y1={sy(hover.y)} x2={w} y2={sy(hover.y)} />
            </g>
          )}
          {target && (
            <g className="stroke-amber-400" strokeWidth={0.6}>
              {tool && <line x1={tool.x} y1={tool.y} x2={sx(target.x)} y2={sy(target.y)} strokeDasharray="2 1.5" />}
              <circle cx={sx(target.x)} cy={sy(target.y)} r={2} fill="none" />
            </g>
          )}
          {tool && (
            <g>
              <circle cx={tool.x} cy={tool.y} r={3.2} className="fill-sky-500/30 stroke-sky-400" strokeWidth={0.6} />
              <circle cx={tool.x} cy={tool.y} r={0.8} className="fill-sky-400" />
            </g>
          )}
          <text x={w / 2} y={h + 6} textAnchor="middle" className="fill-muted-foreground text-[4px]">
            front
          </text>
        </svg>
      </div>
    </div>
  );
}
