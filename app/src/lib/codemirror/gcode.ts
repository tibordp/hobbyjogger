// G-code support for CodeMirror: highlighting, completion, hover help and the editor theme.
import {
  autocompletion,
  type Completion,
  type CompletionContext,
  type CompletionResult,
} from "@codemirror/autocomplete";
import {
  HighlightStyle,
  StreamLanguage,
  syntaxHighlighting,
  type StringStream,
} from "@codemirror/language";
import { EditorView, hoverTooltip, tooltips, type Rect } from "@codemirror/view";
import { tags as t } from "@lezer/highlight";

import type { CodeRef } from "@/bindings/CodeRef";
import { allCodes, lookup } from "@/lib/reference";

/** Width of the completion's info panel, px. Fixed so it doesn't change side from item to item. */
const INFO_WIDTH = 288;

interface LexState {
  /** The letter whose number comes next (for axis and parameter words). */
  pending: string | null;
}

const WORD = /^[a-z]\s*[-+]?(\d+\.?\d*|\.\d+)/i;
const NUMBER = /^\s*[-+]?(\d+\.?\d*|\.\d+)/;

function wordStyle(letter: string) {
  if (letter === "G") return "keyword";
  if (letter === "M") return "atom";
  if (letter === "T") return "typeName";
  if (letter === "N") return "labelName";
  if ("XYZA".includes(letter)) return "variableName";
  if ("IJKR".includes(letter)) return "propertyName";
  return "attributeName";
}

function token(stream: StringStream, state: LexState): string | null {
  if (state.pending) {
    state.pending = null;
    if (stream.match(NUMBER)) return "number";
  }
  if (stream.eatSpace()) return null;
  // Console commands (`$H`, `play …`) at the start of a line.
  if (stream.column() === stream.indentation() && stream.match(/^(\$|[a-z]{2}).*/i)) return "meta";
  if (stream.match(/^\([^)]*\)/)) return "comment";
  if (stream.peek() === "(" || stream.peek() === ";") {
    const unclosed = stream.peek() === "(";
    stream.skipToEnd();
    return unclosed ? "invalid" : "comment";
  }
  if (stream.eat("%")) return "meta";

  const letter = stream.peek()?.toUpperCase();
  if (letter && stream.match(WORD, false)) {
    if (letter === "G" || letter === "M" || letter === "T" || letter === "N") {
      stream.match(WORD);
      return wordStyle(letter);
    }
    stream.next();
    state.pending = letter;
    return wordStyle(letter);
  }
  // A letter at the end of the line is a word still being typed, not an error (yet).
  if (letter && /[A-Z]/.test(letter) && stream.match(/^[a-z]\s*$/i)) return wordStyle(letter);
  stream.next();
  return "invalid";
}

export const gcodeLanguage = StreamLanguage.define<LexState>({
  name: "gcode",
  startState: () => ({ pending: null }),
  token,
  copyState: (s) => ({ ...s }),
  languageData: { commentTokens: { line: ";" } },
});

const highlight = HighlightStyle.define([
  { tag: t.keyword, color: "oklch(0.75 0.14 240)", fontWeight: "600" },
  { tag: t.atom, color: "oklch(0.75 0.15 320)", fontWeight: "600" },
  { tag: t.typeName, color: "oklch(0.8 0.14 80)" },
  { tag: t.labelName, color: "var(--muted-foreground)" },
  { tag: t.variableName, color: "oklch(0.8 0.12 160)" },
  { tag: t.propertyName, color: "oklch(0.78 0.1 200)" },
  { tag: t.attributeName, color: "oklch(0.8 0.12 60)" },
  { tag: t.number, color: "var(--foreground)" },
  { tag: t.comment, color: "var(--muted-foreground)", fontStyle: "italic" },
  { tag: t.meta, color: "oklch(0.75 0.1 280)" },
  { tag: t.invalid, color: "oklch(0.7 0.2 25)", textDecoration: "underline wavy" },
]);

const theme = EditorView.theme(
  {
    "&": { height: "100%", fontSize: "12px", backgroundColor: "transparent" },
    ".cm-scroller": { fontFamily: "ui-monospace, SFMono-Regular, Menlo, monospace", lineHeight: "1.55" },
    ".cm-content": { caretColor: "var(--foreground)" },
    ".cm-cursor": { borderLeftColor: "var(--foreground)" },
    ".cm-gutters": {
      backgroundColor: "transparent",
      color: "var(--muted-foreground)",
      borderRight: "1px solid var(--border)",
    },
    ".cm-activeLine": { backgroundColor: "color-mix(in oklch, var(--muted) 45%, transparent)" },
    ".cm-activeLineGutter": { backgroundColor: "transparent", color: "var(--foreground)" },
    "&.cm-focused .cm-selectionBackground, .cm-selectionBackground, ::selection": {
      backgroundColor: "color-mix(in oklch, oklch(0.6 0.12 240) 35%, transparent) !important",
    },
    ".cm-job-line": { backgroundColor: "color-mix(in oklch, oklch(0.75 0.15 80) 22%, transparent)" },
    ".cm-tooltip": {
      backgroundColor: "var(--popover)",
      color: "var(--popover-foreground)",
      border: "1px solid var(--border)",
      borderRadius: "6px",
    },
    ".cm-tooltip-autocomplete > ul > li[aria-selected]": {
      backgroundColor: "var(--accent)",
      color: "var(--accent-foreground)",
    },
    ".cm-completionDetail": { marginLeft: "0.75em", color: "var(--muted-foreground)", fontStyle: "normal" },
    ".cm-code-help": { padding: "6px 8px", maxWidth: "22rem", fontFamily: "var(--font-sans)" },
    // As specific as CodeMirror's own rule (`max-content`, up to 400 px), which it would win.
    ".cm-tooltip.cm-completionInfo": { width: `${INFO_WIDTH}px`, maxWidth: "none" },
    // CodeMirror builds the panel for each item and places it a frame later; until then it would
    // flash wherever it falls (over the console's input and button).
    ".cm-tooltip.cm-completionInfo:not([class*='cm-completionInfo-'])": { visibility: "hidden" },
    ".cm-completionInfo .cm-code-help": { maxWidth: "none" },
    ".cm-code-help b": { fontFamily: "ui-monospace, monospace" },
    ".cm-code-detail": { color: "var(--muted-foreground)" },
    ".cm-code-params": {
      display: "grid",
      gridTemplateColumns: "auto 1fr",
      columnGap: "0.6em",
      marginTop: "4px",
      fontSize: "11px",
    },
    ".cm-panels": { backgroundColor: "var(--card)", color: "var(--foreground)" },
    ".cm-searchMatch": { backgroundColor: "color-mix(in oklch, oklch(0.8 0.15 90) 30%, transparent)" },
    ".cm-diagnostic": { fontFamily: "var(--font-sans)" },
  },
  { dark: true },
);

/** Where completion runs: the editor offers G/M codes for programs, the console everything. */
export type Scope = "program" | "console";

const inScope = (c: CodeRef, scope: Scope) =>
  scope === "console" || ((c.kind === "g" || c.kind === "m") && c.scope === "both");

/** The help for a code: what it does and its parameters. */
function helpDom(c: CodeRef, withTitle: boolean) {
  const dom = document.createElement("div");
  dom.className = "cm-code-help";
  if (withTitle) {
    const title = document.createElement("div");
    const code = document.createElement("b");
    code.textContent = c.code;
    title.append(code, ` ${c.title}`);
    dom.append(title);
  }
  if (c.detail) {
    const detail = document.createElement("div");
    detail.className = "cm-code-detail";
    detail.textContent = c.detail;
    dom.append(detail);
  }
  if (c.params.length > 0) {
    const params = document.createElement("div");
    params.className = "cm-code-params";
    for (const p of c.params) {
      const name = document.createElement("b");
      name.textContent = p.name;
      const meaning = document.createElement("span");
      meaning.textContent = p.meaning;
      params.append(name, meaning);
    }
    dom.append(params);
  }
  return dom;
}

/** A code as a completion. `order` keeps the table's order (G4 before G32) among equally good
 * matches, which would otherwise sort alphabetically. */
const option = (c: CodeRef, order: number): Completion => ({
  label: c.code,
  detail: c.title,
  // Only when there's more to say than the title, or the panel shows up empty.
  info: c.detail || c.params.length > 0 ? () => helpDom(c, false) : undefined,
  type: c.kind === "g" ? "keyword" : c.kind === "m" ? "function" : "namespace",
  boost: -order / 1000,
});

/** The codes on a line, e.g. for `G1 X5 F` → G1; at the console the command word too. */
function codesIn(text: string, scope: Scope) {
  const found: CodeRef[] = [];
  const first = /^\s*(\S+)/.exec(text)?.[1];
  const head = scope === "console" && first ? lookup(first) : undefined;
  if (head && head.kind !== "g" && head.kind !== "m") found.push(head);
  for (const m of text.matchAll(/[GM]\d+(?:\.\d+)?/gi)) {
    const c = lookup(m[0]);
    if (c && !found.includes(c)) found.push(c);
  }
  return found;
}

const complete = (scope: Scope) => (context: CompletionContext): CompletionResult | null => {
  const line = context.state.doc.lineAt(context.pos);
  const before = line.text.slice(0, context.pos - line.from);

  // The command word at the start of a console line: any code or command.
  if (scope === "console" && /^\s*[\w$#.-]*$/.test(before)) {
    const word = context.matchBefore(/[\w$#.-]*/);
    if (!word || (word.from === word.to && !context.explicit)) return null;
    return { from: word.from, options: allCodes().map(option), validFor: /^[\w$#.-]*$/ };
  }

  const code = context.matchBefore(/[GgMm]\d*\.?\d*/);
  if (code && (code.from === line.from || /[\s\d]/.test(line.text[code.from - line.from - 1]))) {
    if (code.from === code.to && !context.explicit) return null;
    return {
      from: code.from,
      options: allCodes()
        .filter((c) => (c.kind === "g" || c.kind === "m") && inScope(c, scope))
        .map(option),
      validFor: /^[GgMm]\d*\.?\d*$/,
    };
  }

  // A parameter of a code on the line: its letter and what it means. At the console also right
  // after a space, to show what's left to give.
  const word = context.matchBefore(/(?<=^|\s)[A-Za-z]?$/);
  if (!word) return null;
  if (word.from === word.to && !context.explicit && scope !== "console") return null;
  const used = new Set([...before.matchAll(/(?:^|\s)([A-Za-z])[-+.\d]/g)].map((m) => m[1].toUpperCase()));
  const seen = new Set<string>();
  const options: Completion[] = [];
  for (const c of codesIn(before, scope)) {
    for (const p of c.params) {
      if (p.name.length !== 1 || used.has(p.name) || seen.has(p.name)) continue;
      seen.add(p.name);
      options.push({ label: p.name, detail: p.meaning, type: "property" });
    }
  }
  if (options.length === 0) return null;
  return { from: word.from, options, validFor: /^[A-Za-z]?$/ };
};

const help = (scope: Scope) =>
  hoverTooltip((view, pos) => {
    const line = view.state.doc.lineAt(pos);
    const words = [...line.text.matchAll(/[GM]\s*\d+(\.\d+)?/gi)].map((m) => ({ text: m[0], at: m.index }));
    // At the console, the command word too.
    const first = /^(\s*)(\S+)/.exec(line.text);
    if (scope === "console" && first) words.unshift({ text: first[2], at: first[1].length });
    for (const w of words) {
      const from = line.from + w.at;
      const to = from + w.text.length;
      if (pos < from || pos > to) continue;
      const info = lookup(w.text.replace(/\s/g, ""));
      if (!info) continue;
      return { pos: from, end: to, above: true, create: () => ({ dom: helpDom(info, true) }) };
    }
    return null;
  });

/** Where the info panel goes: beside the list (right if it fits, else left), level with its
 * top but kept inside the window, else above it. Decided by a fixed width, so it stays put while
 * moving through the list (CodeMirror's default measures each item's text and moved it around). */
function positionInfo(
  _view: EditorView,
  list: Rect,
  _option: Rect,
  info: Rect,
  space: Rect,
) {
  const width = INFO_WIDTH + 4;
  // Relative to the list's top: 0, or up as far as needed to end inside the window.
  const top = Math.max(space.top - list.top, Math.min(0, space.bottom - (info.bottom - info.top) - list.top - 4));
  if (space.right - list.right >= width) return { style: `top: ${top}px`, class: "cm-completionInfo-right" };
  if (list.left - space.left >= width) return { style: `top: ${top}px`, class: "cm-completionInfo-left" };
  return {
    style: `bottom: ${list.bottom - list.top}px; width: ${Math.min(INFO_WIDTH, space.right - list.left - 30)}px`,
    class: "cm-completionInfo-right-narrow",
  };
}

/** Completion and hover help, from the shared reference. */
export const codeHelp = (scope: Scope) => [
  // The console's line sits at the bottom of the window.
  autocompletion({
    override: [complete(scope)],
    icons: false,
    aboveCursor: scope === "console",
    positionInfo,
  }),
  help(scope),
];

/** Highlighting and the theme, shared by the editor and the console. */
export const gcodeStyle = () => [
  gcodeLanguage,
  syntaxHighlighting(highlight),
  theme,
  // Completion lists and help go on top of everything: inside the editor they'd be clipped by the
  // panels around it (the info panel ended up under the program editor), or make a panel scroll
  // and the console's input jump.
  tooltips({ parent: document.body, position: "fixed" }),
];

export const gcode = () => [...gcodeStyle(), ...codeHelp("program")];
