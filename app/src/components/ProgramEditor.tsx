import { closeBrackets } from "@codemirror/autocomplete";
import { defaultKeymap, history, historyKeymap, indentWithTab } from "@codemirror/commands";
import { bracketMatching } from "@codemirror/language";
import { lintGutter, setDiagnostics, type Diagnostic } from "@codemirror/lint";
import { search, searchKeymap } from "@codemirror/search";
import { Compartment, EditorState, StateEffect, StateField, type Extension } from "@codemirror/state";
import {
  Decoration,
  drawSelection,
  EditorView,
  highlightActiveLine,
  highlightActiveLineGutter,
  keymap,
  lineNumbers,
} from "@codemirror/view";
import { FilePlus2, FolderOpen, Locate, Lock, Play, Save, Upload, VectorSquare } from "lucide-react";
import { useEffect, useRef, useState } from "react";

import { MachineFilesDialog } from "@/components/MachineFilesDialog";
import { RunDialog } from "@/components/RunDialog";
import { TransformSidebar } from "@/components/TransformSidebar";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { act } from "@/lib/actions";
import { gcode } from "@/lib/codemirror/gcode";
import { useConnected, useMachineState } from "@/lib/machine";
import {
  currentText,
  edited,
  formatDuration,
  newProgram,
  openWithDialog,
  registerTextReplacer,
  registerTextSource,
  saveFile,
  takeInitialText,
  useFollowing,
  useProgram,
} from "@/lib/program";
import { useStore } from "@/lib/store";
import { closeTransforms, openTransforms } from "@/lib/transform";
import { useUi } from "@/lib/ui";
import { cn } from "@/lib/utils";

/** Highlights the line a running job has reached. */
const setJobLine = StateEffect.define<number | null>();
const jobLine = StateField.define({
  create: () => Decoration.none,
  update(deco, tr) {
    for (const e of tr.effects) {
      if (e.is(setJobLine)) {
        if (e.value === null || e.value < 1 || e.value > tr.state.doc.lines) {
          return Decoration.none;
        }
        const line = tr.state.doc.line(e.value);
        return Decoration.set([Decoration.line({ class: "cm-job-line" }).range(line.from)]);
      }
    }
    return deco.map(tr.changes);
  },
  provide: (f) => EditorView.decorations.from(f),
});

/** Read-only while following a running job. */
const readOnly = new Compartment();
const lockedWhen = (locked: boolean) => readOnly.reconfigure(EditorState.readOnly.of(locked));

/** Called when the user scrolls the editor themselves (not when we scroll to the job line). */
let onUserScroll = () => {};
const NAV_KEYS = new Set(["ArrowUp", "ArrowDown", "PageUp", "PageDown", "Home", "End"]);

function extensions(): Extension[] {
  return [
    readOnly.of(EditorState.readOnly.of(false)),
    EditorView.domEventHandlers({
      wheel: () => onUserScroll(),
      touchmove: () => onUserScroll(),
      keydown: (e) => {
        if (NAV_KEYS.has(e.key)) onUserScroll();
      },
      // A drag on the scrollbar (clicks in the text land on the content, not the scroller).
      mousedown: (e, view) => {
        if (e.target === view.scrollDOM) onUserScroll();
      },
    }),
    lineNumbers(),
    highlightActiveLineGutter(),
    history(),
    drawSelection(),
    bracketMatching(),
    closeBrackets(),
    highlightActiveLine(),
    search({ top: true }),
    lintGutter(),
    jobLine,
    keymap.of([...defaultKeymap, ...historyKeymap, ...searchKeymap, indentWithTab]),
    gcode(),
    EditorView.updateListener.of((u) => {
      if (u.docChanged) edited();
      if (u.selectionSet || u.docChanged) {
        const line = u.state.doc.lineAt(u.state.selection.main.head).number;
        if (useProgram.getState().cursorLine !== line) useProgram.setState({ cursorLine: line });
      }
    }),
  ];
}

export function ProgramEditor() {
  const host = useRef<HTMLDivElement>(null);
  const view = useRef<EditorView | null>(null);
  const docId = useProgram((s) => s.docId);
  const name = useProgram((s) => s.name);
  const path = useProgram((s) => s.path);
  const dirty = useProgram((s) => s.dirty);
  const analysis = useProgram((s) => s.analysis);
  const analyzedVersion = useProgram((s) => s.analyzedVersion);
  const version = useProgram((s) => s.version);
  const following = useFollowing();
  const job = useStore((s) => s.state?.job ?? null);
  const progress = useStore((s) => s.state?.status?.job ?? null);
  const transfer = useStore((s) => s.state?.transfer ?? null);
  const connected = useConnected();
  const state = useMachineState();
  const [running, setRunning] = useState(false);
  const transforming = useUi((s) => s.transforms !== null);
  /** Keeping the job line in view; off once the user scrolls away. */
  const [live, setLive] = useState(true);

  useEffect(() => {
    const v = new EditorView({ parent: host.current! });
    view.current = v;
    registerTextSource(() => v.state.doc.toString());
    registerTextReplacer((text) => v.dispatch({ changes: { from: 0, to: v.state.doc.length, insert: text } }));
    return () => {
      registerTextSource(takeInitialText);
      registerTextReplacer(null);
      v.destroy();
      view.current = null;
    };
  }, []);

  // A different document: replace the editor state (resets undo history, no change events).
  useEffect(() => {
    view.current?.setState(EditorState.create({ doc: takeInitialText(), extensions: extensions() }));
  }, [docId]);

  // While following a job the text is the job's file, so it can't be edited.
  useEffect(() => {
    view.current?.dispatch({ effects: lockedWhen(following) });
    setLive(true);
  }, [following, docId]);
  useEffect(() => {
    onUserScroll = () => following && setLive(false);
    return () => {
      onUserScroll = () => {};
    };
  }, [following]);

  // Diagnostics from the analysis, if it's for the current text.
  useEffect(() => {
    const v = view.current;
    if (!v || !analysis || analyzedVersion !== version) return;
    const doc = v.state.doc;
    const diagnostics: Diagnostic[] = analysis.diagnostics
      .filter((d) => d.line <= doc.lines)
      .map((d) => {
        const line = doc.line(d.line);
        return { from: line.from, to: line.to, severity: d.severity, message: d.message };
      });
    v.dispatch(setDiagnostics(v.state, diagnostics));
  }, [analysis, analyzedVersion, version]);

  // The job's current line, highlighted and kept in view. The firmware reports line 0 until the
  // first line is read.
  const jobLineNo = following && progress && progress.lines >= 1 ? progress.lines : null;
  useEffect(() => {
    const v = view.current;
    if (!v) return;
    v.dispatch({ effects: setJobLine.of(jobLineNo) });
    if (live && jobLineNo !== null && jobLineNo <= v.state.doc.lines) {
      v.dispatch({
        effects: EditorView.scrollIntoView(v.state.doc.line(jobLineNo).from, { y: "center" }),
      });
    }
  }, [jobLineNo, live]);

  const errors = analysis?.diagnostics.filter((d) => d.severity === "error").length ?? 0;
  const busy = transfer !== null;
  const canUpload = connected && !busy && !job && !progress;

  const upload = async () => {
    const version = useProgram.getState().version;
    if (await act({ type: "upload", name: name.trim(), text: currentText(), run: false })) {
      useProgram.setState({ onMachine: { path: `/sd/gcodes/${name.trim()}`, version } });
    }
  };

  return (
    <div className="flex h-full flex-col">
      <div className="flex h-11 shrink-0 items-center gap-1.5 border-b px-2">
        <Button size="sm" variant="ghost" onClick={() => void newProgram()} title="New program">
          <FilePlus2 />
        </Button>
        <Button size="sm" variant="ghost" onClick={() => void openWithDialog()} title="Open… (⌘O)">
          <FolderOpen />
        </Button>
        <MachineFilesDialog />
        <Button size="sm" variant="ghost" onClick={() => void saveFile()} title={`Save${path ? ` to ${path}` : "…"} (⌘S)`}>
          <Save />
        </Button>
        <Input
          className="h-7 w-56 font-mono text-xs"
          disabled={following}
          value={name}
          onChange={(e) => useProgram.setState({ name: e.target.value })}
          title={path ?? "Name on the machine's SD card"}
        />
        {following ? (
          <span className="flex items-center gap-1 whitespace-nowrap text-xs text-muted-foreground" title="The editor follows the running job; edit again once it's done.">
            <Lock className="size-3" /> running · read-only
          </span>
        ) : (
          dirty && <span className="text-xs text-muted-foreground">edited</span>
        )}
        <div className="mx-1 h-5 w-px shrink-0 bg-border" />
        <Button
          size="sm"
          variant="ghost"
          className={cn(transforming && "bg-muted")}
          aria-pressed={transforming}
          disabled={following || (!transforming && !analysis)}
          onClick={() => (transforming ? closeTransforms() : openTransforms())}
          title="Transform the program: rotate it to line it up with a part that sits turned, or move it"
        >
          <VectorSquare /> Transform
        </Button>
        <div className="flex-1" />
        {analysis && (
          <span className="mr-2 truncate text-xs text-muted-foreground">
            {analysis.lineCount} lines · ≈{formatDuration(analysis.estimatedSecs)}
            {errors > 0 && <span className="text-red-400"> · {errors} error{errors > 1 && "s"}</span>}
          </span>
        )}
        <Button size="sm" variant="outline" disabled={!canUpload} onClick={() => void upload()} title="Upload to the machine's SD card">
          <Upload /> Upload
        </Button>
        <Button
          size="sm"
          disabled={!canUpload || state !== "idle"}
          onClick={() => setRunning(true)}
          title="Upload and run"
        >
          <Play /> Run
        </Button>
      </div>
      <div className="flex min-h-0 flex-1">
        <div className="relative min-h-0 min-w-0 flex-1">
          <div ref={host} className="absolute inset-0 overflow-hidden" />
          {following && !live && (
            <Button size="sm" className="absolute right-5 bottom-4 z-10 shadow-lg" onClick={() => setLive(true)}>
              <Locate /> Follow job{jobLineNo !== null && ` · line ${jobLineNo}`}
            </Button>
          )}
        </div>
        <TransformSidebar />
      </div>
      <RunDialog open={running} onOpenChange={setRunning} />
    </div>
  );
}
