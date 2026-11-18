// F-28: the now-working pill's title as a ticker (parity with mobile/src/ui/Ticker.tsx): a title wider than
// its line scrolls left without end, edges fading into the surface, as the Music app's now-playing title.
// A title that fits stays still. With reduced motion on, or without the Web Animations API, it is cut short
// with an ellipsis instead and never moves.
import { useLayoutEffect, useRef, useState } from "react";
import { cn } from "@/lib/utils";
import { useMediaQuery } from "@/ui/useMediaQuery";
import { REDUCED_MOTION_QUERY } from "./ProjectMascot";

/** Pixels a second, about the pace of the Music app's now-playing title. */
export const TICKER_SPEED = 30;
/** Space between one run of the title and the next. */
export const TICKER_GAP = 56;
/** How far the edges fade into the surface. */
const FADE = 16;

const EDGE_MASK = `linear-gradient(to right, transparent, #000 ${FADE}px, #000 calc(100% - ${FADE}px), transparent)`;

export function Ticker({ text, className }: { text: string; className?: string }) {
  const reduced = useMediaQuery(REDUCED_MOTION_QUERY);
  const boxRef = useRef<HTMLSpanElement>(null);
  const textRef = useRef<HTMLSpanElement>(null);
  const trackRef = useRef<HTMLSpanElement>(null);
  /** The title's width when it is wider than the line, else 0. */
  const [wide, setWide] = useState(0);

  useLayoutEffect(() => {
    const box = boxRef.current;
    const title = textRef.current;
    if (!box || !title) return;
    const measure = () => setWide(title.scrollWidth > box.clientWidth + 1 ? title.scrollWidth : 0);
    measure();
    if (typeof ResizeObserver === "undefined") return;
    const observer = new ResizeObserver(measure);
    observer.observe(box);
    return () => observer.disconnect();
  }, [text]);

  const scrolling = wide > 0 && !reduced;
  useLayoutEffect(() => {
    const track = trackRef.current;
    if (!scrolling || !track || typeof track.animate !== "function") return;
    const distance = wide + TICKER_GAP;
    const animation = track.animate([{ transform: "translateX(0)" }, { transform: `translateX(-${distance}px)` }], {
      duration: (distance / TICKER_SPEED) * 1000,
      iterations: Infinity,
      easing: "linear",
    });
    return () => animation.cancel();
  }, [scrolling, wide, text]);

  return (
    <span ref={boxRef} data-ticker={scrolling ? "scrolling" : "still"} className={cn("block min-w-0 overflow-hidden whitespace-nowrap", className)} style={scrolling ? { maskImage: EDGE_MASK, WebkitMaskImage: EDGE_MASK } : undefined}>
      <span ref={trackRef} className={scrolling ? "flex w-max" : "block"} style={scrolling ? { gap: TICKER_GAP } : undefined}>
        <span ref={textRef} className={scrolling ? "inline-block" : "block truncate"}>
          {text}
        </span>
        {scrolling ? <span aria-hidden="true">{text}</span> : null}
      </span>
    </span>
  );
}
