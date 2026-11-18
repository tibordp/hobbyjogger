import { useEffect, useLayoutEffect, useRef, useState } from "react";
import { createPortal } from "react-dom";

/** Before the first tooltip shows, ms. */
const DELAY = 600;
/** Moving on to another element within this long shows its tooltip at once, ms. */
const SKIP_DELAY = 400;
const GAP = 6;
const MARGIN = 8;

interface Shown {
  text: string;
  rect: DOMRect;
}

/**
 * Shows every `title` attribute in the app as a styled tooltip instead of the browser's own. While
 * an element is hovered its `title` is taken off (so the native one doesn't show too) and put back
 * when the pointer leaves, so React's idea of the attribute stays right. Where pointer events don't
 * arrive (some disabled controls), the native tooltip still shows.
 */
export function TitleTooltips() {
  const [shown, setShown] = useState<Shown | null>(null);

  useEffect(() => {
    let current: { el: Element; title: string } | null = null;
    let timer: number | undefined;
    let lastHidden = 0;

    const restore = () => {
      window.clearTimeout(timer);
      if (current) {
        // Unless React set a new one meanwhile.
        if (!current.el.hasAttribute("title")) current.el.setAttribute("title", current.title);
        current = null;
      }
      setShown((s) => {
        if (s) lastHidden = performance.now();
        return null;
      });
    };

    const show = () => {
      if (!current?.el.isConnected) return;
      setShown({ text: current.title, rect: current.el.getBoundingClientRect() });
    };

    const onOver = (e: PointerEvent) => {
      const el = e.target instanceof Element ? e.target.closest("[title]") : null;
      // Within the element shown (its title is off for now), unless onto a titled one inside it.
      if (current?.el.contains(e.target as Node) && !(el && el !== current.el && current.el.contains(el))) return;
      restore();
      if (!el || el instanceof SVGElement) return;
      const title = el.getAttribute("title");
      if (!title) return;
      el.removeAttribute("title");
      current = { el, title };
      if (performance.now() - lastHidden < SKIP_DELAY) show();
      else timer = window.setTimeout(show, DELAY);
    };
    const onOut = (e: PointerEvent) => {
      if (current && !(e.relatedTarget instanceof Node && current.el.contains(e.relatedTarget))) restore();
    };
    const hide = () => {
      restore();
      lastHidden = 0;
    };

    document.addEventListener("pointerover", onOver);
    document.addEventListener("pointerout", onOut);
    document.addEventListener("pointerdown", hide, true);
    document.addEventListener("keydown", hide, true);
    document.addEventListener("wheel", hide, { capture: true, passive: true });
    window.addEventListener("blur", hide);
    return () => {
      restore();
      document.removeEventListener("pointerover", onOver);
      document.removeEventListener("pointerout", onOut);
      document.removeEventListener("pointerdown", hide, true);
      document.removeEventListener("keydown", hide, true);
      document.removeEventListener("wheel", hide, { capture: true });
      window.removeEventListener("blur", hide);
    };
  }, []);

  return shown && createPortal(<Tip shown={shown} />, document.body);
}

/** Below the element (above if there's no room), kept inside the window. */
function Tip({ shown: { text, rect } }: { shown: Shown }) {
  const ref = useRef<HTMLDivElement>(null);
  const [pos, setPos] = useState<{ left: number; top: number } | null>(null);
  useLayoutEffect(() => {
    const el = ref.current;
    if (!el) return;
    const { width, height } = el.getBoundingClientRect();
    const below = rect.bottom + GAP;
    const top = below + height + MARGIN > window.innerHeight ? rect.top - GAP - height : below;
    const centre = rect.left + rect.width / 2 - width / 2;
    const left = Math.min(Math.max(centre, MARGIN), window.innerWidth - width - MARGIN);
    setPos({ left, top: Math.max(top, MARGIN) });
  }, [text, rect]);
  return (
    <div
      ref={ref}
      role="tooltip"
      className="pointer-events-none fixed z-[1000] w-max max-w-xs rounded-md bg-foreground px-2.5 py-1.5 text-xs whitespace-pre-line text-background shadow-md"
      style={pos ? { left: pos.left, top: pos.top } : { left: 0, top: 0, visibility: "hidden" }}
    >
      {text}
    </div>
  );
}
