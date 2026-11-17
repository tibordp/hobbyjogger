// View-only state shared between panels. Nothing here affects the machine by itself.
import { create } from "zustand";

import type { Across } from "@/bindings/Across";
import type { Corner } from "@/bindings/Corner";
import type { Routine } from "@/bindings/Routine";
import type { Side } from "@/bindings/Side";
import { useStore } from "@/lib/store";
import type { Transforms } from "@/lib/transform";

/** A routine, or a surface map or contour (which aren't: they run long and keep their points). */
export type ProbeKind = Exclude<Routine["kind"], "edgeAngle"> | "map" | "contour";

/** The probing form, kept between uses (and shown as a preview on the bed). */
export interface ProbeForm {
  kind: ProbeKind;
  side: Side;
  corner: Corner;
  /** How far below the top to touch sides, mm. */
  depth: number;
  /** Corner: how far the edges may be from where the probe starts, mm. */
  reach: number;
  /** Edge: how far to search for it, mm. */
  edgeSearch: number;
  /** Edge: touch twice along it for its angle too. */
  edgeAngle: boolean;
  /** Edge angle: how far apart along the edge the two touches are, mm. */
  edgeSpan: number;
  boreDiameter: number;
  bossDiameter: number;
  /** Bore and boss: both axes for a centre, or one for a centreline. */
  across: Across;
  /** Map: the area's front-left and back-right corners, work X/Y; null until picked. */
  mapMin: [number, number] | null;
  mapMax: [number, number] | null;
  /** Map: distance between points, mm (rounded to fit the area evenly). */
  mapSpacing: number;
  /** Map: how far below the start height to search, mm. */
  mapDepth: number;
  /** Map: a search and a slow touch per point instead of one touch. */
  mapPrecise: boolean;
  /** Map: refine the grid where the surface needs it. */
  mapAdaptive: boolean;
  /** Map, adaptive: how many times the spacing may be halved. */
  mapLevels: number;
  /** Map, adaptive: how far a cell's centre may be from its corners' average before it's split, mm. */
  mapTolerance: number;
  /** Contour: a pocket's wall from inside, rather than a part's outline from outside. */
  contourInside: boolean;
  /** Contour, outside: the box around the part, work X/Y; null until picked. */
  contourMin: [number, number] | null;
  contourMax: [number, number] | null;
  /** Contour: between touches along the wall, mm. */
  contourSpacing: number;
  /** Contour: the tightest the wall bends toward the probe, mm. */
  contourRadius: number;
  /** Contour, inside: how far to look for the front wall, mm. */
  contourSearch: number;
}

export type MapView = "2d" | "3d";

interface Ui {
  /** Jog speed as a fraction of the axis max rate; also used for moves picked on the bed. */
  jogSpeed: number;
  probe: ProbeForm;
  /** The bed map, top-down or in 3D; remembered between runs. */
  mapView: MapView;
  /** Surface maps: show and export points corrected for the ball touching slopes off its bottom. */
  mapCompensate: boolean;
  /** The editor's transforms sidebar (rotating and moving the program), while it's open. */
  transforms: Transforms | null;
}

const MAP_VIEW_KEY = "hj.mapView";

function storedMapView(): MapView {
  try {
    return localStorage.getItem(MAP_VIEW_KEY) === "3d" ? "3d" : "2d";
  } catch {
    return "2d";
  }
}

export function setMapView(mapView: MapView) {
  useUi.setState({ mapView });
  try {
    localStorage.setItem(MAP_VIEW_KEY, mapView);
  } catch {
    // Not remembered, then.
  }
}

export const useUi = create<Ui>(() => ({
  jogSpeed: 0.25,
  mapView: storedMapView(),
  mapCompensate: true,
  transforms: null,
  probe: {
    kind: "corner",
    side: "left",
    corner: "frontLeft",
    depth: 3,
    reach: 10,
    edgeSearch: 20,
    edgeAngle: false,
    edgeSpan: 30,
    boreDiameter: 10,
    bossDiameter: 10,
    across: "xy",
    mapMin: null,
    mapMax: null,
    mapSpacing: 5,
    mapDepth: 10,
    mapPrecise: false,
    mapAdaptive: false,
    mapLevels: 3,
    mapTolerance: 0.05,
    contourInside: false,
    contourMin: null,
    contourMax: null,
    contourSpacing: 2,
    contourRadius: 3,
    contourSearch: 30,
  },
}));

// The map and contour areas are kept in work coordinates; when the work origin moves (from the DRO,
// a probe result, the console, anywhere), they'd move with it, perhaps off the bed. So they go.
useStore.subscribe((s, prev) => {
  const [now, before] = [s.state?.status?.workOffset, prev.state?.status?.workOffset];
  if (now && before && (now[0] !== before[0] || now[1] !== before[1])) {
    setProbeForm({ mapMin: null, mapMax: null, contourMin: null, contourMax: null });
  }
});

export function setProbeForm(patch: Partial<ProbeForm>) {
  useUi.setState((s) => ({ probe: { ...s.probe, ...patch } }));
}

/** The routine the form describes; not for maps. */
export function routineOf(f: ProbeForm): Routine {
  switch (f.kind) {
    case "map":
    case "contour":
      throw new Error(`a ${f.kind} isn't a routine`);
    case "surface":
      return { kind: "surface" };
    case "edge":
      return f.edgeAngle
        ? { kind: "edgeAngle", side: f.side, search: f.edgeSearch, span: f.edgeSpan }
        : { kind: "edge", side: f.side, search: f.edgeSearch };
    case "corner":
      return { kind: "corner", corner: f.corner, depth: f.depth, reach: f.reach };
    case "bore":
      return { kind: "bore", diameter: f.boreDiameter, across: f.across };
    case "boss":
      return { kind: "boss", diameter: f.bossDiameter, depth: f.depth, across: f.across };
  }
}
