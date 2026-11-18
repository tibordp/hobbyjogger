import { ChevronDown } from "lucide-react";
import { useEffect, useRef, useState, type ReactNode } from "react";

import { cn } from "@/lib/utils";

/** A panel in a [`ScrollColumn`]; `name` is what the column says is off-screen. */
export function Panel({ name, children }: { name: string; children: ReactNode }) {
  return <section data-panel={name}>{children}</section>;
}

/**
 * A vertically scrolling stack of panels that makes clear when some are out of view: fades at the
 * cut-off edges, and below the column a strip naming the panels that don't fully show, which
 * scrolls to them. macOS hides scrollbars until you scroll, so without this a short window just
 * looks like it has fewer panels.
 */
export function ScrollColumn({ children, className }: { children: ReactNode; className?: string }) {
  const scroller = useRef<HTMLDivElement>(null);
  const [moreAbove, setMoreAbove] = useState(false);
  const [below, setBelow] = useState<{ more: boolean; names: string[] }>({ more: false, names: [] });

  useEffect(() => {
    const el = scroller.current!;
    const update = () => {
      const box = el.getBoundingClientRect();
      const down: string[] = [];
      for (const p of el.querySelectorAll<HTMLElement>("[data-panel]")) {
        if (p.getBoundingClientRect().bottom > box.bottom + 1) down.push(p.dataset.panel!);
      }
      const moreDown = el.scrollTop + el.clientHeight < el.scrollHeight - 1;
      setMoreAbove(el.scrollTop > 1);
      setBelow((s) =>
        s.more === moreDown && s.names.join("\n") === down.join("\n") ? s : { more: moreDown, names: down },
      );
    };
    const resize = new ResizeObserver(update);
    resize.observe(el);
    for (const child of el.children) resize.observe(child);
    el.addEventListener("scroll", update, { passive: true });
    update();
    return () => {
      resize.disconnect();
      el.removeEventListener("scroll", update);
    };
  }, []);

  /** Scroll the first panel that doesn't fully show into view (or a screenful down). */
  const reveal = () => {
    const el = scroller.current!;
    const target = below.names[0] && el.querySelector<HTMLElement>(`[data-panel="${below.names[0]}"]`);
    if (target && target.getBoundingClientRect().height < el.clientHeight) {
      target.scrollIntoView({ behavior: "smooth", block: "end" });
    } else {
      el.scrollBy({ top: el.clientHeight * 0.8, behavior: "smooth" });
    }
  };

  return (
    <div className={cn("flex min-h-0 flex-col", className)}>
      <div className="relative min-h-0 flex-1">
        <div ref={scroller} className="scroll-thin flex h-full flex-col gap-3 overflow-y-auto pr-1 [&>*]:shrink-0">
          {children}
        </div>
        <div
          className={cn(
            "pointer-events-none absolute inset-x-0 top-0 h-8 bg-gradient-to-b from-background to-transparent transition-opacity",
            moreAbove ? "opacity-100" : "opacity-0",
          )}
        />
        <div
          className={cn(
            "pointer-events-none absolute inset-x-0 bottom-0 h-8 bg-gradient-to-t from-background to-transparent transition-opacity",
            below.more ? "opacity-100" : "opacity-0",
          )}
        />
      </div>
      {below.more && (
        <button
          type="button"
          onClick={reveal}
          className="mt-1 flex shrink-0 items-center justify-center gap-1 rounded-md py-1 text-xs text-muted-foreground hover:bg-muted hover:text-foreground"
          title="Scroll down, or make the window taller to see everything"
        >
          <ChevronDown className="size-3.5" />
          <span className="truncate">{below.names.length > 0 ? `More: ${below.names.join(", ")}` : "More"}</span>
        </button>
      )}
    </div>
  );
}
