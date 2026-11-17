// Transforming the program in the editor (the sidebar's cards): what each would do to a point,
// for the dashed preview on the maps. The text itself is rewritten in Rust (`hj_gcode::transform`).
import { useUi } from "@/lib/ui";

export interface Transforms {
  rotate: { degrees: number; pivot: [number, number] };
  translate: [number, number, number];
  /** The card the maps show the result of: the one changed last. */
  preview: "rotate" | "translate";
}

export const NO_TRANSFORMS: Transforms = { rotate: { degrees: 0, pivot: [0, 0] }, translate: [0, 0, 0], preview: "rotate" };

export function openTransforms() {
  useUi.setState((s) => ({ transforms: s.transforms ?? NO_TRANSFORMS }));
}
export function closeTransforms() {
  useUi.setState({ transforms: null });
}
/** Open the sidebar with the rotation set to `degrees` (about where it was turning about). */
export function startRotate(degrees: number) {
  useUi.setState((s) => {
    const t = s.transforms ?? NO_TRANSFORMS;
    return { transforms: { ...t, rotate: { ...t.rotate, degrees }, preview: "rotate" } };
  });
}
export function setRotate(patch: Partial<Transforms["rotate"]>) {
  useUi.setState((s) => (s.transforms ? { transforms: { ...s.transforms, rotate: { ...s.transforms.rotate, ...patch }, preview: "rotate" } } : {}));
}
export function setTranslate(translate: [number, number, number]) {
  useUi.setState((s) => (s.transforms ? { transforms: { ...s.transforms, translate, preview: "translate" } } : {}));
}

/** What the previewed card would do to a program point (X/Y/Z), or null while it does nothing. */
export function previewOf(t: Transforms | null): ((x: number, y: number, z: number) => [number, number, number]) | null {
  if (!t) return null;
  if (t.preview === "translate") {
    const [dx, dy, dz] = t.translate;
    if (dx === 0 && dy === 0 && dz === 0) return null;
    return (x, y, z) => [x + dx, y + dy, z + dz];
  }
  const { degrees, pivot } = t.rotate;
  if (degrees === 0) return null;
  const [sin, cos] = [Math.sin((degrees * Math.PI) / 180), Math.cos((degrees * Math.PI) / 180)];
  return (x, y, z) => {
    const [dx, dy] = [x - pivot[0], y - pivot[1]];
    return [dx * cos - dy * sin + pivot[0], dx * sin + dy * cos + pivot[1], z];
  };
}
