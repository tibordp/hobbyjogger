// What each G/M code and console command does on the Z1, as the backend knows it (one table, in
// `hj_gcode::reference`), for the editor's and the console's completion and help.
import type { CodeRef } from "@/bindings/CodeRef";
import { gcodeReference } from "@/lib/backend";

let codes: CodeRef[] = [];
let byCode = new Map<string, CodeRef>();

/** Fetch the table. Call once at startup; until it arrives, completion offers nothing. */
export async function loadReference() {
  codes = await gcodeReference();
  byCode = new Map(codes.map((c) => [c.code, c]));
}

export const allCodes = () => codes;

/** `G01`, `g1` and `G38.20` as G1 and G38.2; other words as typed. Mirrors the Rust side. */
export function normalise(word: string) {
  const m = /^([GM])(\d+)(?:\.(\d*))?$/i.exec(word.trim());
  if (!m) return word.trim();
  const int = m[1].toUpperCase() + (m[2].replace(/^0+/, "") || "0");
  const frac = (m[3] ?? "").replace(/0+$/, "");
  return frac ? `${int}.${frac}` : int;
}

export const lookup = (word: string): CodeRef | undefined => byCode.get(normalise(word));

/** How it's written: "M220 S<Percent>", "config-get sd <key>". */
export function usage(c: CodeRef) {
  const args = c.params.map((p) =>
    p.name.startsWith("<") || c.kind === "shell" ? p.name : `${p.name}…`,
  );
  return [c.code, ...args].join(" ");
}
