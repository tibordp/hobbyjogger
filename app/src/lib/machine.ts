import type { MachineState } from "@/bindings/MachineState";
import { useStore } from "@/lib/store";

export const useConnected = () => useStore((s) => s.state?.connection.kind === "connected");
export const useStatus = () => useStore((s) => s.state?.status ?? null);
export const useMachineState = (): MachineState | null =>
  useStore((s) => s.state?.status?.state ?? null);
export const useJobRunning = () => useStore((s) => s.state?.status?.job != null);

export const STATE_LABEL: Record<MachineState, string> = {
  idle: "Idle",
  run: "Running",
  hold: "Hold",
  home: "Homing",
  alarm: "Alarm",
  sleep: "Sleep",
  pause: "Paused",
  wait: "Waiting",
  tool: "Tool change",
  unknown: "Unknown",
};

/** The 3D probe's tool number on the Z1. */
export const PROBE_TOOL = 9999;
/** The depth probe's: it only touches down, so it measures Z only. */
export const DEPTH_PROBE_TOOL = 0;
/** The laser module's: laser mode changes to it. */
export const LASER_TOOL = 8888;

export function toolName(n: number) {
  if (n === PROBE_TOOL) return "3D probe";
  if (n === DEPTH_PROBE_TOOL) return "Depth probe";
  if (n === LASER_TOOL) return "Laser";
  if (n < 0) return "none";
  return `T${n}`;
}

/** The tool as it reads mid-sentence: "the 3D probe", "T3". */
export function theTool(n: number) {
  if (n === PROBE_TOOL) return "the 3D probe";
  if (n === DEPTH_PROBE_TOOL) return "the depth probe";
  if (n === LASER_TOOL) return "the laser module";
  return toolName(n);
}

export function isProbe(n: number | null | undefined) {
  return n === PROBE_TOOL || n === DEPTH_PROBE_TOOL;
}

/** Mirrors the core's gating so controls can be disabled; the core remains the authority. */
export function canJog(state: MachineState | null, job: boolean) {
  return !job && (state === "idle" || state === "run");
}

/** Machine − work for X, Y, Z (tool length included), from the live position. */
export function useWorkOffset(): [number, number, number] | null {
  const t = useStore((s) => s.telemetry);
  const connected = useStore((s) => s.state?.connection.kind === "connected");
  if (!t || !connected) return null;
  return [t.machine[0] - t.work[0], t.machine[1] - t.work[1], t.machine[2] - t.work[2]];
}
