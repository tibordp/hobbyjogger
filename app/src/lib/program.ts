// The G-code program open in the editor. The editor buffer is a frontend concern; reading,
// writing and analysing it happen in Rust (see docs/architecture.md).
import { ask, open, save } from "@tauri-apps/plugin-dialog";
import { toast } from "sonner";
import { create } from "zustand";

import type { Analysis } from "@/bindings/Analysis";
import type { Program } from "@/bindings/Program";
import type { Travel } from "@/bindings/Travel";
import { analyzeGcode, downloadProgram, openProgram, saveProgram } from "@/lib/backend";
import { load as loadSetting, save as saveSetting } from "@/lib/persist";
import { useStore } from "@/lib/store";

export const GCODE_EXTENSIONS = ["nc", "gcode", "ngc", "gc", "tap", "cnc", "txt"];
const ANALYZE_DELAY_MS = 300;

interface ProgramStore {
  /** Local file, if any. */
  path: string | null;
  /** Name for the machine's SD card. */
  name: string;
  /** Bumped when a different document is loaded, so the editor replaces its contents. */
  docId: number;
  /** Bumped on every edit. */
  version: number;
  dirty: boolean;
  analysis: Analysis | null;
  /** The `version` that `analysis` describes. */
  analyzedVersion: number;
  /** 1-based line of the editor cursor. */
  cursorLine: number | null;
  /** The file on the machine this document is a copy of (uploaded or downloaded), as of
   * `version`. A job running that file can be followed in the editor. */
  onMachine: { path: string; version: number } | null;
}

export const useProgram = create<ProgramStore>(() => ({
  path: null,
  name: "untitled.nc",
  docId: 0,
  version: 0,
  dirty: false,
  analysis: null,
  analyzedVersion: -1,
  cursorLine: null,
  onMachine: null,
}));

/** Text of a freshly loaded document, picked up by the editor when `docId` changes. */
let initialText = "";
/** Reads the editor's current text; registered by the editor. */
let textSource: () => string = () => initialText;

/** Replaces the editor's text as one undoable edit; registered by the editor. */
let textReplacer: ((text: string) => void) | null = null;

export const takeInitialText = () => initialText;
export function registerTextReplacer(f: ((text: string) => void) | null) {
  textReplacer = f;
}
/** Swap the whole program for `text` (one undo step back to what it was). */
export function replaceText(text: string) {
  textReplacer?.(text);
}
export function registerTextSource(f: () => string) {
  textSource = f;
}
export const currentText = () => textSource();

let analyzeTimer: ReturnType<typeof setTimeout> | undefined;

async function analyzeNow(text = currentText()) {
  const version = useProgram.getState().version;
  try {
    const analysis = await analyzeGcode(text);
    // A newer edit may have landed meanwhile; its own analysis will follow.
    if (useProgram.getState().version === version) {
      useProgram.setState({ analysis, analyzedVersion: version });
    }
  } catch (e) {
    toast.error(`Couldn't analyse the program: ${e}`);
  }
}

/** Called by the editor on every change. */
export function edited() {
  useProgram.setState((s) => ({ version: s.version + 1, dirty: true }));
  clearTimeout(analyzeTimer);
  analyzeTimer = setTimeout(() => void analyzeNow(), ANALYZE_DELAY_MS);
}

function load(text: string, path: string | null, name: string) {
  initialText = text;
  clearTimeout(analyzeTimer);
  useProgram.setState((s) => ({
    path,
    name,
    docId: s.docId + 1,
    version: s.version + 1,
    dirty: false,
    analysis: null,
    cursorLine: null,
    onMachine: null,
  }));
  // The editor picks the text up on its next render, so analyse it directly.
  void analyzeNow(text);
}

const RECENT_MAX = 10;

/** Local files opened or saved lately, most recent first (File ▸ Open Recent). */
export const useRecent = create<{ paths: string[] }>(() => ({
  paths: loadSetting<string[]>("recentFiles", []).filter((p) => typeof p === "string"),
}));
useRecent.subscribe((s) => saveSetting("recentFiles", s.paths));

function addRecent(path: string) {
  useRecent.setState((s) => ({ paths: [path, ...s.paths.filter((p) => p !== path)].slice(0, RECENT_MAX) }));
}

export const forgetRecent = (path?: string) =>
  useRecent.setState((s) => ({ paths: path === undefined ? [] : s.paths.filter((p) => p !== path) }));

/** Ask before throwing away unsaved edits. */
async function confirmDiscard(): Promise<boolean> {
  const { dirty, name } = useProgram.getState();
  if (!dirty) return true;
  return ask(`${name} has unsaved changes. Discard them?`, {
    title: "Unsaved changes",
    kind: "warning",
    okLabel: "Discard",
    cancelLabel: "Cancel",
  });
}

export async function newProgram() {
  if (!(await confirmDiscard())) return;
  load("", null, "untitled.nc");
}

export async function openPath(path: string) {
  if (!(await confirmDiscard())) return;
  try {
    const p: Program = await openProgram(path);
    load(p.text, p.path, p.name);
    addRecent(path);
  } catch (e) {
    toast.error(String(e));
    // Moved or deleted, most likely.
    forgetRecent(path);
  }
}

/**
 * Open a file from the machine's SD card in the editor, unless the editor already has the same
 * text (then nothing is downloaded). Returns whether the editor now holds that file.
 */
export async function openRemote(path: string): Promise<boolean> {
  try {
    const p = await downloadProgram(path, currentText());
    if (p !== null) {
      if (!(await confirmDiscard())) return false;
      load(p.text, null, p.name);
    }
    useProgram.setState((s) => ({ onMachine: { path, version: s.version } }));
    return true;
  } catch (e) {
    toast.error(String(e));
    return false;
  }
}

/** Whether the editor holds, unchanged, the file of the job we're running. */
export function useFollowing() {
  const job = useStore((s) => s.state?.job ?? null);
  const onMachine = useProgram((s) => s.onMachine);
  const version = useProgram((s) => s.version);
  return job !== null && onMachine?.path === job.path && onMachine.version === version;
}

export async function openWithDialog() {
  const path = await open({
    multiple: false,
    directory: false,
    filters: [{ name: "G-code", extensions: GCODE_EXTENSIONS }],
  });
  if (path) await openPath(path);
}

export async function saveFile(saveAs = false) {
  const { path: current, name } = useProgram.getState();
  let path = current;
  if (saveAs || !path) {
    path = await save({
      defaultPath: name,
      filters: [{ name: "G-code", extensions: GCODE_EXTENSIONS }],
    });
    if (!path) return;
  }
  try {
    const version = useProgram.getState().version;
    await saveProgram(path, currentText());
    const fileName = path.split(/[\\/]/).pop() ?? name;
    useProgram.setState((s) => ({
      path,
      name: fileName,
      // Edits made while saving keep the document dirty.
      dirty: s.version !== version,
    }));
    addRecent(path);
    toast.success(`Saved ${fileName}`);
  } catch (e) {
    toast.error(String(e));
  }
}

export const isGcodePath = (path: string) =>
  GCODE_EXTENSIONS.includes(path.split(".").pop()?.toLowerCase() ?? "");

export interface FitProblem {
  /** e.g. "Z 4.20 mm too high" */
  short: string;
  /** With the machine coordinates involved. */
  detail: string;
}

/** Which way past the limit, per axis: [below min, above max]. */
const PAST: [string, string][] = [
  ["too far left", "too far right"],
  ["too far forward", "too far back"],
  ["too low", "too high"],
];

/**
 * Where the program would take the head with the current work offset (rapids included), checked
 * against the soft limits. Empty if it fits.
 */
export function fitProblems(a: Analysis, workOffset: number[], travel: Travel): FitProblem[] {
  if (!a.bounds) return [];
  const problems: FitProblem[] = [];
  const add = (i: number, side: 0 | 1, reach: number, limit: number) => {
    const short = `${"XYZ"[i]} ${Math.abs(reach - limit).toFixed(2)} mm ${PAST[i][side]}`;
    problems.push({
      short,
      detail: `${short}: the program reaches machine ${"XYZ"[i]}${reach.toFixed(2)}, the limit is ${limit}`,
    });
  };
  for (let i = 0; i < 3; i++) {
    const lo = a.bounds.min[i] + workOffset[i];
    const hi = a.bounds.max[i] + workOffset[i];
    const min = travel.min[i];
    if (min !== null && lo < min) add(i, 0, lo, min);
    if (hi > travel.max[i]) add(i, 1, hi, travel.max[i]);
  }
  return problems;
}

export function formatDuration(secs: number) {
  const s = Math.round(secs);
  const h = Math.floor(s / 3600);
  const m = Math.floor((s % 3600) / 60);
  if (h > 0) return `${h} h ${m} min`;
  if (m > 0) return `${m} min ${s % 60} s`;
  return `${s} s`;
}
