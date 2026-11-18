import { Copy, Eraser, Search, X } from "lucide-react";
import { memo, useDeferredValue, useEffect, useMemo, useRef, useState } from "react";
import { toast } from "sonner";

import type { ConsoleEntry } from "@/bindings/ConsoleEntry";
import type { ConsoleKind } from "@/bindings/ConsoleKind";
import { ConsoleInput } from "@/components/ConsoleInput";
import { Button } from "@/components/ui/button";
import { useStore } from "@/lib/store";
import { cn } from "@/lib/utils";

const KIND_STYLE: Record<ConsoleKind, string> = {
  sent: "text-sky-400",
  received: "text-foreground",
  error: "text-red-400",
  info: "text-muted-foreground italic",
};

// One formatter: `toLocaleTimeString` builds a new one every call, which adds up.
const timeFormat = new Intl.DateTimeFormat([], { hour: "2-digit", minute: "2-digit", second: "2-digit", hour12: false });

/** A line never changes once logged, so only new ones render (the log runs to thousands of lines,
 * and a probing routine adds several a second). */
const Row = memo(function Row({ entry: e }: { entry: ConsoleEntry }) {
  return (
    <div className={cn("whitespace-pre-wrap", KIND_STYLE[e.kind])}>
      <span className="mr-2 text-muted-foreground/60">{timeFormat.format(e.time)}</span>
      {e.kind === "sent" && "> "}
      {e.text}
    </div>
  );
});

const asText = (entries: ConsoleEntry[]) =>
  entries.map((e) => `${timeFormat.format(e.time)} ${e.kind === "sent" ? "> " : ""}${e.text}`).join("\n");

export function ConsolePanel() {
  const all = useStore((s) => s.console);
  const scroller = useRef<HTMLDivElement>(null);
  const pinned = useRef(true);
  const [query, setQuery] = useState("");
  // Typing stays quick with thousands of lines to filter.
  const q = useDeferredValue(query.trim().toLowerCase());
  const entries = useMemo(() => (q ? all.filter((e) => e.text.toLowerCase().includes(q)) : all), [all, q]);

  useEffect(() => {
    const el = scroller.current;
    if (el && pinned.current) el.scrollTop = el.scrollHeight;
  }, [entries]);

  const copy = async () => {
    try {
      await navigator.clipboard.writeText(asText(entries));
      toast.success(`Copied ${entries.length} line${entries.length === 1 ? "" : "s"}`);
    } catch (e) {
      toast.error(`Couldn't copy: ${String(e)}`);
    }
  };

  return (
    <div className="flex h-full flex-col">
      <div className="flex h-11 shrink-0 items-center gap-1.5 border-b px-2">
        <div className="relative min-w-0 flex-1">
          <Search className="pointer-events-none absolute top-1/2 left-2 size-3.5 -translate-y-1/2 text-muted-foreground" />
          <input
            value={query}
            onChange={(e) => setQuery(e.target.value)}
            onKeyDown={(e) => e.key === "Escape" && setQuery("")}
            placeholder="Search the console"
            spellCheck={false}
            className="h-7 w-full rounded-md border border-input bg-transparent pr-7 pl-7 text-xs outline-none placeholder:text-muted-foreground focus-visible:border-ring focus-visible:ring-[3px] focus-visible:ring-ring/50"
          />
          {query && (
            <button
              className="absolute top-1/2 right-1.5 -translate-y-1/2 rounded p-0.5 text-muted-foreground hover:text-foreground"
              onClick={() => setQuery("")}
              title="Clear the search"
            >
              <X className="size-3.5" />
            </button>
          )}
        </div>
        {q && (
          <span className="shrink-0 text-xs text-muted-foreground tabular-nums">
            {entries.length} of {all.length}
          </span>
        )}
        <Button size="sm" variant="ghost" onClick={() => void copy()} disabled={entries.length === 0} title={q ? "Copy the matching lines" : "Copy the console"}>
          <Copy />
        </Button>
        <Button size="sm" variant="ghost" onClick={() => useStore.setState({ console: [] })} disabled={all.length === 0} title="Clear the console">
          <Eraser />
        </Button>
      </div>
      <div
        ref={scroller}
        className="min-h-0 flex-1 overflow-y-auto px-3 py-2 font-mono text-xs leading-5 select-text"
        onScroll={(e) => {
          const el = e.currentTarget;
          pinned.current = el.scrollHeight - el.scrollTop - el.clientHeight < 24;
        }}
      >
        {entries.map((e) => (
          <Row key={e.id} entry={e} />
        ))}
        {all.length > 0 && entries.length === 0 && <div className="text-muted-foreground">Nothing matches “{query.trim()}”.</div>}
      </div>
      <ConsoleInput />
    </div>
  );
}
