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

/** Mirrors the core's gating so controls can be disabled; the core remains the authority. */
export function canJog(state: MachineState | null, job: boolean) {
  return !job && (state === "idle" || state === "run");
}
