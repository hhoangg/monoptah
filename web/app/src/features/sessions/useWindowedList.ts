// A small window-scroll virtualizer for rows of one fixed height (the session cards). It renders only
// the rows near the viewport and a spacer for the rest. No dependency: rows are uniform, the page
// scrolls, and jsdom (no layout) gets a usable window from `innerHeight`.
import { useCallback, useEffect, useLayoutEffect, useRef, useState, type RefObject } from "react";

export interface WindowedRange {
  start: number;
  end: number;
  /** Space above the first rendered row and below the last, in px. */
  before: number;
  after: number;
}

/** Returns the ref to attach to the element that holds the rows (the `ul`), and the range to render. */
export function useWindowedList(count: number, rowHeight: number, overscan = 6): [RefObject<HTMLUListElement | null>, WindowedRange] {
  const ref = useRef<HTMLUListElement | null>(null);
  const [range, setRange] = useState({ first: 0, last: Math.min(count, 24) });

  const measure = useCallback(() => {
    const element = ref.current;
    const viewport = window.innerHeight || 800;
    const top = element ? -element.getBoundingClientRect().top : 0;
    const first = Math.max(0, Math.floor(top / rowHeight) - overscan);
    const last = Math.min(count, Math.ceil((top + viewport) / rowHeight) + overscan);
    setRange((previous) => (previous.first === first && previous.last === last ? previous : { first, last }));
  }, [count, rowHeight, overscan]);

  useLayoutEffect(measure, [measure]);
  useEffect(() => {
    let frame = 0;
    const onChange = () => {
      cancelAnimationFrame(frame);
      frame = requestAnimationFrame(measure);
    };
    window.addEventListener("scroll", onChange, { passive: true });
    window.addEventListener("resize", onChange);
    return () => {
      cancelAnimationFrame(frame);
      window.removeEventListener("scroll", onChange);
      window.removeEventListener("resize", onChange);
    };
  }, [measure]);

  const start = Math.min(range.first, count);
  const end = Math.min(range.last, count);
  return [ref, { start, end, before: start * rowHeight, after: (count - end) * rowHeight }];
}
