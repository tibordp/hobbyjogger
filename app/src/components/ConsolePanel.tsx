import { memo, useEffect, useRef } from "react";

import type { ConsoleEntry } from "@/bindings/ConsoleEntry";
import type { ConsoleKind } from "@/bindings/ConsoleKind";
import { ConsoleInput } from "@/components/ConsoleInput";
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

export function ConsolePanel() {
  const entries = useStore((s) => s.console);
  const scroller = useRef<HTMLDivElement>(null);
  const pinned = useRef(true);

  useEffect(() => {
    const el = scroller.current;
    if (el && pinned.current) el.scrollTop = el.scrollHeight;
  }, [entries]);

  return (
    <div className="flex h-full flex-col">
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
      </div>
      <ConsoleInput />
    </div>
  );
}
