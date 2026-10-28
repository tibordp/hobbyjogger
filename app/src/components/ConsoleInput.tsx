// The console's command line: a small CodeMirror, so it highlights, completes and explains codes
// and commands from the same reference as the editor. Enter sends, Shift+Enter (or a paste) adds
// lines, which the core sends one by one; ↑/↓ on the first/last line go through history.
import { acceptCompletion, completionStatus, startCompletion } from "@codemirror/autocomplete";
import { defaultKeymap, history as undoHistory, historyKeymap, insertNewline } from "@codemirror/commands";
import { Compartment, EditorState, Prec } from "@codemirror/state";
import { EditorView, keymap, placeholder } from "@codemirror/view";
import { CornerDownLeft, Square } from "lucide-react";
import { useEffect, useRef, useState } from "react";

import type { CodeRef } from "@/bindings/CodeRef";
import { Button } from "@/components/ui/button";
import { act } from "@/lib/actions";
import { codeHelp, gcodeStyle } from "@/lib/codemirror/gcode";
import { useConnected } from "@/lib/machine";
import { useStore } from "@/lib/store";
import { lookup, usage } from "@/lib/reference";

const editable = new Compartment();
const hint = new Compartment();

/** Grows with its lines up to about ten, then scrolls. */
const look = Prec.high(
  EditorView.theme({
    "&": { height: "auto", maxHeight: "11.5em", fontSize: "12px" },
    ".cm-content": { padding: "5px 0" },
    ".cm-line": { padding: "0 8px" },
    ".cm-scroller": { overflow: "auto" },
    "&.cm-focused": { outline: "none" },
    // The list opens above the line; clear the input's border and padding instead of covering it.
    ".cm-tooltip-autocomplete.cm-tooltip-above": { marginTop: "-8px" },
  }),
);

/** The code or command the cursor is in or after, for the hint under the line. */
function current(state: EditorState): CodeRef | undefined {
  const head = state.selection.main.head;
  const line = state.doc.lineAt(head);
  const text = line.text;
  const before = text.slice(0, head - line.from);
  const codes = [...before.matchAll(/[GM]\d+(?:\.\d+)?/gi)];
  const last = codes.at(-1);
  if (last) {
    // The word being typed, or the last one before the cursor.
    const c = lookup(last[0]);
    if (c) return c;
  }
  const first = /^\s*(\S+)/.exec(text)?.[1];
  return first ? lookup(first) : undefined;
}

export function ConsoleInput() {
  const connected = useConnected();
  const host = useRef<HTMLDivElement>(null);
  const view = useRef<EditorView | null>(null);
  const history = useRef<string[]>([]);
  const cursor = useRef<number | null>(null);
  const [empty, setEmpty] = useState(true);
  const [code, setCode] = useState<CodeRef | undefined>();
  const [lines, setLines] = useState(1);
  const batch = useStore((s) => s.state?.batch ?? null);
  const sendRef = useRef<() => void>(() => {});

  useEffect(() => {
    const set = (v: EditorView, text: string) =>
      v.dispatch({ changes: { from: 0, to: v.state.doc.length, insert: text }, selection: { anchor: text.length } });

    const send = (v: EditorView) => {
      const line = v.state.doc.toString().trim();
      if (!line) return true;
      void act({ type: "mdi", line }).then((ok) => {
        if (!ok) return;
        history.current = [...history.current.filter((x) => x !== line), line].slice(-100);
        cursor.current = null;
        set(v, "");
      });
      return true;
    };

    const recall = (v: EditorView, delta: number) => {
      // Let the completion list take the arrows while it's open, and move between lines.
      if (completionStatus(v.state) !== null) return false;
      const at = v.state.doc.lineAt(v.state.selection.main.head).number;
      if (delta < 0 ? at > 1 : at < v.state.doc.lines) return false;
      const h = history.current;
      if (h.length === 0) return true;
      const c = cursor.current;
      const next = c === null ? (delta < 0 ? h.length - 1 : null) : Math.min(Math.max(c + delta, 0), h.length);
      if (next === null || next === h.length) {
        cursor.current = null;
        set(v, "");
      } else {
        cursor.current = next;
        set(v, h[next]);
      }
      return true;
    };

    const v = new EditorView({
      parent: host.current!,
      state: EditorState.create({
        extensions: [
          ...gcodeStyle(),
          ...codeHelp("console"),
          look,
          EditorView.lineWrapping,
          editable.of(EditorView.editable.of(false)),
          hint.of(placeholder("Not connected")),
          undoHistory(),
          keymap.of([
            { key: "Enter", run: send },
            { key: "Shift-Enter", run: insertNewline },
            { key: "Tab", run: (v) => acceptCompletion(v) || startCompletion(v) },
            { key: "ArrowUp", run: (v) => recall(v, -1) },
            { key: "ArrowDown", run: (v) => recall(v, 1) },
            ...defaultKeymap,
            ...historyKeymap,
          ]),
          EditorView.updateListener.of((u) => {
            if (u.docChanged) {
              setEmpty(u.state.doc.toString().trim() === "");
              setLines(u.state.doc.toString().split("\n").filter((l) => l.trim()).length);
            }
            if (u.docChanged || u.selectionSet) setCode(current(u.state));
          }),
        ],
      }),
    });
    view.current = v;
    sendRef.current = () => send(v);
    return () => {
      v.destroy();
      view.current = null;
    };
  }, []);

  useEffect(() => {
    view.current?.dispatch({
      effects: [
        editable.reconfigure(EditorView.editable.of(connected)),
        hint.reconfigure(
          placeholder(
            connected ? "G-code or command" : "Not connected",
          ),
        ),
      ],
    });
  }, [connected]);

  return (
    <div className="border-t px-2 pb-2">
      {/* Always one line, so nothing moves as you type: what's being sent, else the usage of the
          code at the cursor, else the keys. */}
      <div className="flex h-7 min-w-0 items-center gap-2 px-1 text-xs text-muted-foreground">
        {batch ? (
          <>
            <span className="truncate text-sky-400">
              Sending line {Math.min(batch.done + 1, batch.total)} of {batch.total}
            </span>
            <Button
              size="xs"
              variant="ghost"
              className="h-5 shrink-0"
              title="Send no more lines. What the machine has queued still runs; HALT stops it."
              onClick={() => act({ type: "stopBatch" })}
            >
              <Square /> Stop sending
            </Button>
          </>
        ) : code ? (
          <span className="truncate">
            <span className="font-mono text-foreground/80">{usage(code)}</span> · {code.title}
          </span>
        ) : (
          <span className="truncate">Tab completes · ⇧↵ new line · ↑↓ history</span>
        )}
      </div>
      <div className="flex items-end gap-2">
        <div
          ref={host}
          className="min-w-0 flex-1 rounded-md border border-input font-mono focus-within:border-ring focus-within:ring-[3px] focus-within:ring-ring/50"
        />
        {/* Fixed width: "Send" and "Send 12" mustn't squeeze the input. */}
        <Button
          size="sm"
          className="mb-px w-22 shrink-0"
          disabled={!connected || empty || batch !== null}
          onClick={() => sendRef.current()}
        >
          <CornerDownLeft /> {lines > 1 ? `Send ${lines}` : "Send"}
        </Button>
      </div>
    </div>
  );
}
