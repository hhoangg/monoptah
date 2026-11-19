// A small virtualizer for rows of one fixed height (the session cards). It renders only the rows near
// the viewport and a spacer for the rest. It follows the nearest scrollable ancestor of the list (the
// pane of the app shell, or a column of the split layout) and falls back to the window when the
// document itself scrolls. No dependency: rows are uniform, and jsdom (no layout) gets a usable
// window from `innerHeight`.
import { useCallback, useEffect, useLayoutEffect, useRef, useState, type RefObject } from "react";

export interface WindowedRange {
  start: number;
  end: number;
  /** Space above the first rendered row and below the last, in px. */
  before: number;
  after: number;
}

/** The nearest ancestor that scrolls vertically, or null when the document scrolls. */
export function findScrollParent(element: HTMLElement | null): HTMLElement | null {
  for (let node = element?.parentElement ?? null; node && node !== document.body && node !== document.documentElement; node = node.parentElement) {
    const overflow = getComputedStyle(node).overflowY;
    if (overflow === "auto" || overflow === "scroll" || overflow === "overlay") return node;
  }
  return null;
}

/** Returns the ref to attach to the element that holds the rows (the `ul`), and the range to render. */
/**
 * `layoutKey` changes whenever content above the list changes height (a folder section folding, appearing or going),
 * so the range is measured again at once instead of after the next scroll.
 */
export function useWindowedList(count: number, rowHeight: number, overscan = 6, layoutKey = ""): [RefObject<HTMLUListElement | null>, WindowedRange] {
  const ref = useRef<HTMLUListElement | null>(null);
  const [range, setRange] = useState({ first: 0, last: Math.min(count, 24) });

  const measure = useCallback(() => {
    const element = ref.current;
    const container = findScrollParent(element);
    const viewport = (container ? container.clientHeight : 0) || window.innerHeight || 800;
    // Distance the list has scrolled past the top edge of the scrolling viewport.
    const edge = container ? container.getBoundingClientRect().top : 0;
    const top = element ? edge - element.getBoundingClientRect().top : 0;
    const first = Math.max(0, Math.floor(top / rowHeight) - overscan);
    const last = Math.min(count, Math.ceil((top + viewport) / rowHeight) + overscan);
    setRange((previous) => (previous.first === first && previous.last === last ? previous : { first, last }));
  }, [count, rowHeight, overscan]);

  useLayoutEffect(measure, [measure, layoutKey]);
  useEffect(() => {
    let frame = 0;
    const onChange = () => {
      cancelAnimationFrame(frame);
      frame = requestAnimationFrame(measure);
    };
    // Capture on the document: scroll does not bubble, and the scrolling element changes when the layout
    // toggles between split and stacked, so one listener sees whichever pane scrolls.
    document.addEventListener("scroll", onChange, { capture: true, passive: true });
    window.addEventListener("resize", onChange);
    const container = findScrollParent(ref.current);
    const observer = container && typeof ResizeObserver !== "undefined" ? new ResizeObserver(onChange) : null;
    if (container) observer?.observe(container);
    return () => {
      cancelAnimationFrame(frame);
      document.removeEventListener("scroll", onChange, { capture: true });
      window.removeEventListener("resize", onChange);
      observer?.disconnect();
    };
  }, [measure]);

  const start = Math.min(range.first, count);
  const end = Math.min(range.last, count);
  return [ref, { start, end, before: start * rowHeight, after: (count - end) * rowHeight }];
}
