import {
  useCallback,
  useEffect,
  useRef,
  useState,
  type ReactNode,
} from "react";

type Fade = "none" | "start" | "end" | "both";

const EDGE = "1.5rem";
const MASKS: Record<Exclude<Fade, "none">, string> = {
  end: `linear-gradient(to right, black calc(100% - ${EDGE}), transparent)`,
  start: `linear-gradient(to right, transparent, black ${EDGE})`,
  both: `linear-gradient(to right, transparent, black ${EDGE}, black calc(100% - ${EDGE}), transparent)`,
};

/**
 * A horizontally scrolling wrapper for a wide table. Edges that hide more
 * content fade out, so a table clipped on a narrow window reads as scrollable;
 * it takes focus so the arrow keys scroll it.
 */
export function ScrollX({
  label,
  children,
}: {
  label: string;
  children: ReactNode;
}) {
  const box = useRef<HTMLDivElement>(null);
  const [fade, setFade] = useState<Fade>("none");

  const measure = useCallback(() => {
    const el = box.current;
    if (!el) return;
    const hiddenStart = el.scrollLeft > 1;
    const hiddenEnd = el.scrollLeft + el.clientWidth < el.scrollWidth - 1;
    setFade(
      hiddenStart && hiddenEnd
        ? "both"
        : hiddenStart
          ? "start"
          : hiddenEnd
            ? "end"
            : "none",
    );
  }, []);

  useEffect(() => {
    measure();
    const el = box.current;
    if (!el || typeof ResizeObserver === "undefined") return;
    const observer = new ResizeObserver(measure);
    observer.observe(el);
    if (el.firstElementChild) observer.observe(el.firstElementChild);
    return () => observer.disconnect();
  }, [measure]);

  const mask = fade === "none" ? undefined : MASKS[fade];
  return (
    <div
      ref={box}
      role="region"
      aria-label={label}
      tabIndex={0}
      data-fade={fade}
      onScroll={measure}
      style={mask ? { maskImage: mask, WebkitMaskImage: mask } : undefined}
      className="overflow-x-auto focus-visible:outline-2 focus-visible:outline-accent"
    >
      {children}
    </div>
  );
}
