// Typed access to the Rust side. All state and decisions live there; see docs/architecture.md.
import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";

import type { Analysis } from "@/bindings/Analysis";
import type { CodeRef } from "@/bindings/CodeRef";
import type { ConsoleEntry } from "@/bindings/ConsoleEntry";
import type { Drawing } from "@/bindings/Drawing";
import type { Fit } from "@/bindings/Fit";
import type { Program } from "@/bindings/Program";
import type { Request } from "@/bindings/Request";
import type { Snapshot } from "@/bindings/Snapshot";
import type { StateUpdate } from "@/bindings/StateUpdate";
import type { Telemetry } from "@/bindings/Telemetry";

/** Send a request to the core. Rejects with the core's reason if it's refused. */
export function request(request: Request): Promise<void> {
  return invoke("request", { request });
}

export const getState = () => invoke<Snapshot>("get_state");
export const analyzeGcode = (text: string) => invoke<Analysis>("analyze_gcode", { text });
/** The program rotated about Z; rejects with the lines that keep it from being (`Problem[]`). */
export const rotateGcode = (text: string, degrees: number, pivot: [number, number]) =>
  invoke<string>("rotate_gcode", { text, degrees, pivot });
/** The program moved by `offset`; rejects with the lines that keep it from being (`Problem[]`). */
export const translateGcode = (text: string, offset: [number, number, number]) =>
  invoke<string>("translate_gcode", { text, offset });
/** The outlines in a DXF drawing, in mm. */
export const importDxf = (path: string) => invoke<Drawing>("import_dxf", { path });
/** The ways a drawing's outline fits traced ball centres (machine X/Y), best first; `offset` is
 * the ball's radius, negative for a hole traced from inside. */
export const fitOutline = (reference: [number, number][], measured: [number, number][], offset: number) =>
  invoke<Fit[]>("fit_outline", { reference, measured, offset });
export const gcodeReference = () => invoke<CodeRef[]>("gcode_reference");
export const openProgram = (path: string) => invoke<Program>("open_program", { path });
export const saveProgram = (path: string, text: string) =>
  invoke<void>("save_program", { path, text });
/** A program on the machine, or `null` if it's identical to `have`. */
export const downloadProgram = (path: string, have: string | null) =>
  invoke<Program | null>("download_program", { path, have });
export const consoleHistory = () => invoke<ConsoleEntry[]>("console_history");

export const onStateSync = (f: (u: StateUpdate) => void): Promise<UnlistenFn> =>
  listen<StateUpdate>("state-sync", (e) => f(e.payload));
export const onTelemetry = (f: (t: Telemetry) => void): Promise<UnlistenFn> =>
  listen<Telemetry>("telemetry", (e) => f(e.payload));
export const onConsole = (f: (entries: ConsoleEntry[]) => void): Promise<UnlistenFn> =>
  listen<ConsoleEntry[]>("console", (e) => f(e.payload));
