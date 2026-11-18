// Contours on the frontend: the spec the form describes, and the wall from the traced points
// (compensated as in the export). The tracing runs in the core (`hj_core::contour`); this mirrors
// its `wall`: keep them in step.
import type { ContourSpec } from "@/bindings/ContourSpec";
import type { ProbeForm } from "@/lib/ui";

export type V2 = [number, number];

/** The contour the form describes (machine coordinates, as the box is kept). */
export function plannedContour(form: ProbeForm): ContourSpec | null {
  const base = { spacing: form.contourSpacing, radius: form.contourRadius, search: form.contourSearch };
  if (form.contourInside) return { inside: true, min: [0, 0], max: [0, 0], ...base };
  if (!form.contourMin || !form.contourMax) return null;
  const [x0, y0] = form.contourMin;
  const [x1, y1] = form.contourMax;
  if (!(x1 > x0 && y1 > y0)) return null;
  return { inside: false, min: [x0, y0], max: [x1, y1], ...base };
}

/**
 * The wall at each point: with `compensate`, each ball centre moved a ball radius toward the
 * material, square to the line through its neighbours (the material is on the left of the way
 * round, inside or out). Mirrors `hj_core::contour::wall`.
 */
export function wallPoints(points: V2[], ballRadius: number, compensate: boolean, closed: boolean): V2[] {
  const n = points.length;
  if (!compensate || n < 3) return points;
  return points.map((p, i) => {
    // While tracing, the ends have only one neighbour: use the line to it.
    const a = closed || i > 0 ? points[(i + n - 1) % n] : p;
    const b = closed || i < n - 1 ? points[(i + 1) % n] : p;
    const dx = b[0] - a[0];
    const dy = b[1] - a[1];
    const l = Math.hypot(dx, dy) || 1;
    return [p[0] - (ballRadius * dy) / l, p[1] + (ballRadius * dx) / l];
  });
}

/** Length round, and area inside (positive), of a closed outline. */
export function measure(points: V2[]): { perimeter: number; area: number } {
  let perimeter = 0;
  let twice = 0;
  for (let i = 0; i < points.length; i++) {
    const a = points[i];
    const b = points[(i + 1) % points.length];
    perimeter += Math.hypot(b[0] - a[0], b[1] - a[1]);
    twice += a[0] * b[1] - b[0] * a[1];
  }
  return { perimeter, area: Math.abs(twice) / 2 };
}
