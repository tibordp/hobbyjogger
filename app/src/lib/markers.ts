// Markers: points worth remembering (a feature of the part, a clamp to keep clear of, where the
// last job started), listed in the Markers panel and shown on the maps. Each is kept in machine
// coordinates (it stays put on the bed) or work ones (it moves with the origin, like the part's
// features). They're the window's alone, remembered between runs.
import { create } from "zustand";

import { load, save } from "@/lib/persist";

export type Frame = "machine" | "work";

export interface Marker {
  id: number;
  /** Blank until given one; only named markers are labelled on the maps. */
  name: string;
  frame: Frame;
  /** X, Y, Z in `frame`'s coordinates; Z is null for a point without one (picked on the map). */
  pos: [number, number, number | null];
}

export const useMarkers = create<{ markers: Marker[]; /** Hovered in the list: drawn stronger on the maps. */ hovered: number | null }>(() => ({
  markers: load<Marker[]>("markers", []).filter((m) => Array.isArray(m?.pos) && (m.frame === "machine" || m.frame === "work")),
  hovered: null,
}));
useMarkers.subscribe((s, prev) => s.markers !== prev.markers && save("markers", s.markers));

/** What the list and the maps call it: its name, or "Marker 3". */
export const markerLabel = (m: Marker, i: number) => m.name.trim() || `Marker ${i + 1}`;

const r4 = (v: number) => Math.round(v * 1e4) / 1e4;

/** Where it is in machine coordinates; null for a work marker while there's no work offset
 * (machine − work, tool length included in Z). */
export function machinePos(m: Marker, offset: readonly number[] | null): [number, number, number | null] | null {
  if (m.frame === "machine") return m.pos;
  if (!offset) return null;
  const [x, y, z] = m.pos;
  return [r4(x + offset[0]), r4(y + offset[1]), z === null ? null : r4(z + offset[2])];
}

/** Its coordinates in `frame`, or null if that needs a work offset there isn't. */
export function posIn(m: Marker, frame: Frame, offset: readonly number[] | null): [number, number, number | null] | null {
  if (m.frame === frame) return m.pos;
  const p = machinePos(m, offset);
  if (!p || frame === "machine") return p;
  if (!offset) return null;
  const [x, y, z] = p;
  return [r4(x - offset[0]), r4(y - offset[1]), z === null ? null : r4(z - offset[2])];
}

/**
 * Add a marker at `machine` (machine coordinates), kept in work coordinates when there's a work
 * offset (it's most likely a feature of the part), machine ones otherwise. Returns its id.
 */
export function addMarker(machine: [number, number, number | null], offset: readonly number[] | null, name = ""): number {
  const id = Math.max(0, ...useMarkers.getState().markers.map((m) => m.id)) + 1;
  const m: Marker = { id, name, frame: "machine", pos: machine.map((v) => (v === null ? null : r4(v))) as Marker["pos"] };
  const work = offset ? posIn(m, "work", offset) : null;
  useMarkers.setState((s) => ({ markers: [...s.markers, work ? { ...m, frame: "work", pos: work } : m] }));
  return id;
}

export function updateMarker(id: number, patch: Partial<Omit<Marker, "id">>) {
  useMarkers.setState((s) => ({ markers: s.markers.map((m) => (m.id === id ? { ...m, ...patch } : m)) }));
}

/** Keep it in `frame` instead, where it is now. */
export function setFrame(id: number, frame: Frame, offset: readonly number[] | null) {
  const m = useMarkers.getState().markers.find((m) => m.id === id);
  const pos = m && posIn(m, frame, offset);
  if (pos) updateMarker(id, { frame, pos });
}

export function removeMarker(id: number) {
  useMarkers.setState((s) => ({ markers: s.markers.filter((m) => m.id !== id) }));
}
