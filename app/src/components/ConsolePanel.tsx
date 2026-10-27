import { CornerDownLeft } from "lucide-react";
import { useEffect, useRef, useState } from "react";

import type { ConsoleKind } from "@/bindings/ConsoleKind";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { act } from "@/lib/actions";
import { useConnected } from "@/lib/machine";
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
  const connected = useConnected();
  const [line, setLine] = useState("");
  const [history, setHistory] = useState<string[]>([]);
  const [cursor, setCursor] = useState<number | null>(null);
  const scroller = useRef<HTMLDivElement>(null);
  const pinned = useRef(true);

  useEffect(() => {
    const el = scroller.current;
    if (el && pinned.current) el.scrollTop = el.scrollHeight;
  }, [entries]);

  const send = async () => {
    const l = line.trim();
    if (!l) return;
    if (await act({ type: "mdi", line: l })) {
      setHistory((h) => [...h.filter((x) => x !== l), l].slice(-100));
      setLine("");
      setCursor(null);
    }
  };

  const recall = (delta: number) => {
    if (history.length === 0) return;
    const next =
      cursor === null
        ? delta < 0
          ? history.length - 1
          : null
        : Math.min(Math.max(cursor + delta, 0), history.length);
    if (next === null || next === history.length) {
      setCursor(null);
      setLine("");
    } else {
      setCursor(next);
      setLine(history[next]);
    }
  };

  return (
    <div className="flex h-full flex-col">
      <div
        ref={scroller}
        className="min-h-0 flex-1 overflow-y-auto px-3 py-2 font-mono text-xs leading-5"
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
      <div className="flex gap-2 border-t p-2">
        <Input
          className="h-8 font-mono"
          placeholder={connected ? "G-code or console command (↑/↓ history)" : "Not connected"}
          disabled={!connected}
          value={line}
          onChange={(e) => setLine(e.target.value)}
          onKeyDown={(e) => {
            if (e.key === "Enter") void send();
            else if (e.key === "ArrowUp") {
              e.preventDefault();
              recall(-1);
            } else if (e.key === "ArrowDown") {
              e.preventDefault();
              recall(1);
            }
          }}
        />
        <Button size="sm" disabled={!connected || !line.trim()} onClick={send}>
          <CornerDownLeft /> Send
        </Button>
      </div>
    </div>
  );
}
