// View-only state shared between panels. Nothing here affects the machine by itself.
import { create } from "zustand";

import type { Across } from "@/bindings/Across";
import type { Corner } from "@/bindings/Corner";
import type { Routine } from "@/bindings/Routine";
import type { Side } from "@/bindings/Side";

export type ProbeKind = Routine["kind"];

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
  boreDiameter: number;
  bossDiameter: number;
  /** Bore and boss: both axes for a centre, or one for a centreline. */
  across: Across;
}

interface Ui {
  /** Jog speed as a fraction of the axis max rate; also used for moves picked on the bed. */
  jogSpeed: number;
  probe: ProbeForm;
}

export const useUi = create<Ui>(() => ({
  jogSpeed: 0.25,
  probe: {
    kind: "corner",
    side: "left",
    corner: "frontLeft",
    depth: 3,
    reach: 10,
    edgeSearch: 20,
    boreDiameter: 10,
    bossDiameter: 10,
    across: "xy",
  },
}));

export function setProbeForm(patch: Partial<ProbeForm>) {
  useUi.setState((s) => ({ probe: { ...s.probe, ...patch } }));
}

export function routineOf(f: ProbeForm): Routine {
  switch (f.kind) {
    case "surface":
      return { kind: "surface" };
    case "edge":
      return { kind: "edge", side: f.side, search: f.edgeSearch };
    case "corner":
      return { kind: "corner", corner: f.corner, depth: f.depth, reach: f.reach };
    case "bore":
      return { kind: "bore", diameter: f.boreDiameter, across: f.across };
    case "boss":
      return { kind: "boss", diameter: f.bossDiameter, depth: f.depth, across: f.across };
  }
}
