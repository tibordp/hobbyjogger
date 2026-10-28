import { useEffect, useRef } from "react";

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

const time = (ms: number) => new Date(ms).toLocaleTimeString([], { hour12: false });

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
          <div key={e.id} className={cn("whitespace-pre-wrap", KIND_STYLE[e.kind])}>
            <span className="mr-2 text-muted-foreground/60">{time(e.time)}</span>
            {e.kind === "sent" && "> "}
            {e.text}
          </div>
        ))}
      </div>
      <ConsoleInput />
    </div>
  );
}
