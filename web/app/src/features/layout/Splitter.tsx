// A keyboard-accessible splitter between two panes (WAI-ARIA window splitter): arrow keys resize by 16px
// (Shift: 48px), Home and End jump to the limits, a pointer drag resizes continuously.
import { useRef, type KeyboardEvent, type PointerEvent } from "react";
import { cn } from "@/lib/utils";

const STEP = 16;
const BIG_STEP = 48;

export interface SplitterProps {
  /** Accessible name, for example "Resize sidebar". */
  label: string;
  value: number;
  min: number;
  max: number;
  onChange: (width: number) => void;
  /** `1` when dragging right grows the pane (a pane left of the splitter), `-1` for a pane to its right. */
  grow: 1 | -1;
  className?: string;
}

export function Splitter({ label, value, min, max, onChange, grow, className }: SplitterProps) {
  const drag = useRef<{ x: number; start: number } | null>(null);

  const onKeyDown = (event: KeyboardEvent<HTMLDivElement>) => {
    const step = event.shiftKey ? BIG_STEP : STEP;
    let next: number | null = null;
    if (event.key === "ArrowRight") next = value + grow * step;
    else if (event.key === "ArrowLeft") next = value - grow * step;
    else if (event.key === "Home") next = min;
    else if (event.key === "End") next = max;
    if (next === null) return;
    event.preventDefault();
    onChange(Math.min(max, Math.max(min, next)));
  };

  const onPointerDown = (event: PointerEvent<HTMLDivElement>) => {
    drag.current = { x: event.clientX, start: value };
    event.currentTarget.setPointerCapture?.(event.pointerId);
  };
  const onPointerMove = (event: PointerEvent<HTMLDivElement>) => {
    if (!drag.current) return;
    onChange(drag.current.start + grow * (event.clientX - drag.current.x));
  };
  const onPointerUp = () => {
    drag.current = null;
  };

  return (
    <div
      role="separator"
      aria-orientation="vertical"
      aria-label={label}
      aria-valuenow={value}
      aria-valuemin={min}
      aria-valuemax={max}
      tabIndex={0}
      onKeyDown={onKeyDown}
      onPointerDown={onPointerDown}
      onPointerMove={onPointerMove}
      onPointerUp={onPointerUp}
      onPointerCancel={onPointerUp}
      className={cn(
        "group relative z-10 -mx-1 w-2 shrink-0 cursor-col-resize touch-none outline-none",
        "after:absolute after:inset-y-0 after:left-1/2 after:w-px after:-translate-x-1/2 after:bg-stroke hover:after:w-0.5 hover:after:bg-brand focus-visible:after:w-0.5 focus-visible:after:bg-brand",
        className,
      )}
    />
  );
}
