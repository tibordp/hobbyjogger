import { Ellipsis, MapPin, Plus, Trash2 } from "lucide-react";
import { useState } from "react";

import { PointActions } from "@/components/PointActions";
import { Button } from "@/components/ui/button";
import { Card, CardContent, CardHeader, CardTitle } from "@/components/ui/card";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuSeparator,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import { act } from "@/lib/actions";
import { useConnected, useJobRunning, useMachineState, useWorkOffset } from "@/lib/machine";
import {
  addMarker,
  machinePos,
  markerLabel,
  removeMarker,
  setFrame,
  updateMarker,
  useMarkers,
  type Marker,
} from "@/lib/markers";
import { useStore } from "@/lib/store";
import { useUi } from "@/lib/ui";
import { cn } from "@/lib/utils";

/** Points worth remembering: added at the head here, or from the map (a click, or a probed point). */
export function MarkersPanel() {
  const markers = useMarkers((s) => s.markers);
  const connected = useConnected();
  const t = useStore((s) => s.telemetry);
  const offset = useWorkOffset();

  const addHere = () => {
    if (t) addMarker([t.machine[0], t.machine[1], t.machine[2]], offset);
  };

  return (
    <Card size="sm">
      <CardHeader className="flex flex-row items-center gap-2">
        <CardTitle className="flex-1">Markers</CardTitle>
        <Button size="xs" variant="ghost" disabled={!connected || !t} onClick={addHere} title="Add a marker where the tool tip is">
          <Plus /> At the head
        </Button>
      </CardHeader>
      <CardContent>
        {markers.length === 0 ? (
          <div className="flex gap-3">
            <MapPin className="mt-0.5 size-6 shrink-0 text-amber-400" strokeWidth={1.25} />
            <p className="text-xs leading-relaxed text-muted-foreground">
              Points to come back to. Add one where the head is, or click the map (or a probed point) and pick Add
              marker. Named markers are labelled on the map.
            </p>
          </div>
        ) : (
          <div className="-mx-1 space-y-0.5">
            {markers.map((m, i) => (
              <MarkerRow key={m.id} marker={m} index={i} offset={offset} />
            ))}
          </div>
        )}
      </CardContent>
    </Card>
  );
}

function MarkerRow({ marker: m, index, offset }: { marker: Marker; index: number; offset: [number, number, number] | null }) {
  const connected = useConnected();
  const state = useMachineState();
  const job = useJobRunning();
  const speed = useUi((s) => s.jogSpeed);
  const [name, setName] = useState<string | null>(null);
  const machine = machinePos(m, offset);
  const canAct = connected && state === "idle" && !job;
  const work = m.frame === "work";
  const [x, y, z] = m.pos;
  const fmt = (v: number) => v.toFixed(3);

  return (
    <div
      className="group rounded-md px-1 py-1 hover:bg-muted/50"
      onMouseEnter={() => useMarkers.setState({ hovered: m.id })}
      onMouseLeave={() => useMarkers.setState({ hovered: null })}
    >
      <div className="flex items-center gap-1">
        <MapPin className="size-3.5 shrink-0 text-amber-400" />
        <input
          className="h-6 min-w-0 flex-1 rounded border border-transparent bg-transparent px-1 text-sm outline-none placeholder:text-muted-foreground hover:border-input focus:border-ring"
          value={name ?? m.name}
          placeholder={markerLabel({ ...m, name: "" }, index)}
          spellCheck={false}
          onChange={(e) => setName(e.target.value)}
          onBlur={() => {
            if (name !== null) updateMarker(m.id, { name: name.trim() });
            setName(null);
          }}
          onKeyDown={(e) => {
            if (e.key === "Enter") e.currentTarget.blur();
            if (e.key === "Escape") {
              setName(null);
              requestAnimationFrame(() => (e.target as HTMLInputElement).blur());
            }
          }}
        />
        <button
          type="button"
          className={cn(
            "h-5 w-6 shrink-0 rounded border text-[10px] font-semibold",
            work ? "border-sky-500/40 text-sky-400" : "border-border text-muted-foreground",
            "disabled:opacity-50",
          )}
          disabled={!offset}
          onClick={() => setFrame(m.id, work ? "machine" : "work", offset)}
          title={
            work
              ? "Work coordinates: it moves with the work origin. Click to keep it where it is on the bed instead."
              : "Machine coordinates: it stays where it is on the bed. Click to have it move with the work origin instead."
          }
        >
          {work ? "W" : "M"}
        </button>
        <DropdownMenu>
          <DropdownMenuTrigger asChild>
            <Button size="icon-xs" variant="ghost" className="shrink-0" title="More">
              <Ellipsis />
            </Button>
          </DropdownMenuTrigger>
          <DropdownMenuContent align="end" className="min-w-60">
            {machine && (
              <>
                <PointActions
                  pos={machine}
                  workOffset={offset}
                  canAct={canAct}
                  speed={speed}
                  onMove={(x, y) => void act({ type: "moveTo", x, y, speed })}
                />
                <DropdownMenuSeparator />
              </>
            )}
            <DropdownMenuItem variant="destructive" onSelect={() => removeMarker(m.id)}>
              <Trash2 /> Delete marker
            </DropdownMenuItem>
          </DropdownMenuContent>
        </DropdownMenu>
      </div>
      <div className="pl-5 font-mono text-xs text-muted-foreground select-text">
        X{fmt(x)} Y{fmt(y)}
        {z !== null && <> Z{fmt(z)}</>}
      </div>
    </div>
  );
}
