import { ArrowDownToLine, Copy, Crosshair, Navigation } from "lucide-react";
import { toast } from "sonner";

import { DropdownMenuItem, DropdownMenuSeparator, DropdownMenuShortcut } from "@/components/ui/dropdown-menu";
import { act } from "@/lib/actions";

export type Pos = readonly [number, number, number | null];

/** "X1.000 Y2.000 Z3.000" relative to `origin` (machine − work for work coordinates, zeros for
 * machine ones); no Z for a point without one. */
export function coordsText([x, y, z]: Pos, origin: readonly number[]) {
  return `X${(x - origin[0]).toFixed(3)} Y${(y - origin[1]).toFixed(3)}${z !== null ? ` Z${(z - origin[2]).toFixed(3)}` : ""}`;
}

const copy = (text: string) =>
  navigator.clipboard.writeText(text).then(
    () => toast(`Copied ${text}`),
    () => toast.error("Couldn't copy to the clipboard"),
  );

/**
 * What can be done with a point (machine coordinates), as menu items: make it the work origin,
 * move the head above it, copy its coordinates. For the map's points and markers alike.
 */
export function PointActions({
  pos,
  workOffset,
  canAct,
  speed,
  onMove,
  isOrigin = false,
}: {
  pos: Pos;
  workOffset: readonly number[] | null;
  canAct: boolean;
  /** Fraction of the max rate, shown beside Move. */
  speed: number;
  onMove: (x: number, y: number) => void;
  /** It's the work origin already: nothing to set. */
  isOrigin?: boolean;
}) {
  const [x, y, z] = pos;
  return (
    <>
      {!isOrigin && z !== null && (
        <DropdownMenuItem disabled={!canAct} onSelect={() => void act({ type: "setOrigin", x, y, z })}>
          <Crosshair /> Set work origin here
          <DropdownMenuShortcut>X/Y/Z</DropdownMenuShortcut>
        </DropdownMenuItem>
      )}
      {!isOrigin && (
        <DropdownMenuItem disabled={!canAct} onSelect={() => void act({ type: "setOrigin", x, y })}>
          <Crosshair /> Set work origin here
          <DropdownMenuShortcut>X/Y</DropdownMenuShortcut>
        </DropdownMenuItem>
      )}
      {!isOrigin && z !== null && (
        <DropdownMenuItem disabled={!canAct} onSelect={() => void act({ type: "setOrigin", z })}>
          <ArrowDownToLine /> Set work Z zero here
          <DropdownMenuShortcut>Z</DropdownMenuShortcut>
        </DropdownMenuItem>
      )}
      {!isOrigin && <DropdownMenuSeparator />}
      <DropdownMenuItem disabled={!canAct} onSelect={() => onMove(x, y)}>
        <Navigation /> Move head above it
        <DropdownMenuShortcut>{Math.round(speed * 100)}%</DropdownMenuShortcut>
      </DropdownMenuItem>
      <DropdownMenuSeparator />
      {workOffset && (
        <DropdownMenuItem onSelect={() => void copy(coordsText(pos, workOffset))}>
          <Copy /> Copy work coordinates
        </DropdownMenuItem>
      )}
      <DropdownMenuItem onSelect={() => void copy(coordsText(pos, [0, 0, 0]))}>
        <Copy /> Copy machine coordinates
      </DropdownMenuItem>
    </>
  );
}
