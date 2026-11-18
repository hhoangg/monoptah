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
  /** Called while resizing (every key press or pointer move): update the layout, do not persist. */
  onChange: (width: number) => void;
  /** Called once a resize ends (key press, pointer up): persist the width. */
  onCommit?: (width: number) => void;
  /** `1` when dragging right grows the pane (a pane left of the splitter), `-1` for a pane to its right. */
  grow: 1 | -1;
  className?: string;
}

export function Splitter({ label, value, min, max, onChange, onCommit, grow, className }: SplitterProps) {
  const drag = useRef<{ x: number; start: number; last: number } | null>(null);

  const onKeyDown = (event: KeyboardEvent<HTMLDivElement>) => {
    const step = event.shiftKey ? BIG_STEP : STEP;
    let next: number | null = null;
    if (event.key === "ArrowRight") next = value + grow * step;
    else if (event.key === "ArrowLeft") next = value - grow * step;
    else if (event.key === "Home") next = min;
    else if (event.key === "End") next = max;
    if (next === null) return;
    event.preventDefault();
    const clamped = Math.min(max, Math.max(min, next));
    onChange(clamped);
    onCommit?.(clamped);
  };

  const onPointerDown = (event: PointerEvent<HTMLDivElement>) => {
    drag.current = { x: event.clientX, start: value, last: value };
    event.currentTarget.setPointerCapture?.(event.pointerId);
  };
  const onPointerMove = (event: PointerEvent<HTMLDivElement>) => {
    if (!drag.current) return;
    const next = Math.min(max, Math.max(min, drag.current.start + grow * (event.clientX - drag.current.x)));
    drag.current.last = next;
    onChange(next);
  };
  const onPointerUp = () => {
    const finished = drag.current;
    drag.current = null;
    if (finished && finished.last !== finished.start) onCommit?.(finished.last);
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
