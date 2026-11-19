import { useEffect, useLayoutEffect, useState, type RefObject } from "react";

/**
 * The content width of `ref` in CSS px, kept current with a ResizeObserver, or 0 until it is measured (and where layout or
 * ResizeObserver do not exist, as in jsdom). Used to choose a layout from the pane width instead of the viewport width.
 */
export function useElementWidth(ref: RefObject<HTMLElement | null>): number {
  const [width, setWidth] = useState(0);
  // Measure before the first paint, so a wide pane never flashes (and remounts) its narrow layout first.
  useLayoutEffect(() => {
    const element = ref.current;
    if (element) setWidth(element.getBoundingClientRect().width);
  }, [ref]);
  useEffect(() => {
    const element = ref.current;
    if (!element) return;
    if (typeof ResizeObserver !== "function") return;
    const observer = new ResizeObserver((entries) => {
      const entry = entries[entries.length - 1];
      if (entry) setWidth(entry.contentRect.width);
    });
    observer.observe(element);
    return () => observer.disconnect();
  }, [ref]);
  return width;
}
