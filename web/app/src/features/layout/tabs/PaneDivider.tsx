// The divider between two panes of a split: drag it (pointer) or use the arrow keys, like the sidebar Splitter, but in
// fractions of the split's size, because a pane tree has no pixel widths of its own.
import { useRef, type KeyboardEvent, type PointerEvent } from "react";
import { cn } from "@/lib/utils";
import { MIN_PANE_HEIGHT, MIN_PANE_WIDTH, type SplitDir } from "./paneTree";

const STEP = 0.02;
const BIG_STEP = 0.06;

export interface PaneDividerProps {
  dir: SplitDir;
  /** The sizes of every child of the split and the index of the child before this divider. */
  sizes: number[];
  index: number;
  label: string;
  /** Called while dragging (`commit` false) and when a resize ends (`commit` true). */
  onResize: (sizes: number[], commit: boolean) => void;
}

export function PaneDivider({ dir, sizes, index, label, onResize }: PaneDividerProps) {
  const drag = useRef<{ start: number; total: number; sizes: number[]; last: number[] } | null>(null);
  const min = dir === "row" ? MIN_PANE_WIDTH : MIN_PANE_HEIGHT;

  const shifted = (from: number[], delta: number, total: number): number[] => {
    const low = total > 0 ? Math.min(0.5, min / total) : 0.1;
    const before = from[index] ?? 0;
    const after = from[index + 1] ?? 0;
    const move = Math.min(Math.max(delta, low - before), after - low);
    const next = [...from];
    next[index] = before + move;
    next[index + 1] = after - move;
    return next;
  };

  const containerSize = (element: HTMLElement): number => {
    const rect = element.parentElement?.getBoundingClientRect();
    return (dir === "row" ? rect?.width : rect?.height) ?? 0;
  };

  const onKeyDown = (event: KeyboardEvent<HTMLDivElement>) => {
    const grow = dir === "row" ? "ArrowRight" : "ArrowDown";
    const shrink = dir === "row" ? "ArrowLeft" : "ArrowUp";
    const step = event.shiftKey ? BIG_STEP : STEP;
    let delta: number | null = null;
    if (event.key === grow) delta = step;
    else if (event.key === shrink) delta = -step;
    if (delta === null) return;
    event.preventDefault();
    onResize(shifted(sizes, delta, containerSize(event.currentTarget)), true);
  };

  const onPointerDown = (event: PointerEvent<HTMLDivElement>) => {
    drag.current = { start: dir === "row" ? event.clientX : event.clientY, total: containerSize(event.currentTarget), sizes, last: sizes };
    event.currentTarget.setPointerCapture?.(event.pointerId);
  };
  const onPointerMove = (event: PointerEvent<HTMLDivElement>) => {
    const state = drag.current;
    if (!state || state.total <= 0) return;
    const delta = ((dir === "row" ? event.clientX : event.clientY) - state.start) / state.total;
    state.last = shifted(state.sizes, delta, state.total);
    onResize(state.last, false);
  };
  const onPointerUp = () => {
    const state = drag.current;
    drag.current = null;
    if (state && state.last !== state.sizes) onResize(state.last, true);
  };

  const percent = Math.round((sizes[index] ?? 0) * 100);
  return (
    <div
      role="separator"
      aria-orientation={dir === "row" ? "vertical" : "horizontal"}
      aria-label={label}
      aria-valuenow={percent}
      aria-valuemin={0}
      aria-valuemax={100}
      tabIndex={0}
      onKeyDown={onKeyDown}
      onPointerDown={onPointerDown}
      onPointerMove={onPointerMove}
      onPointerUp={onPointerUp}
      onPointerCancel={onPointerUp}
      className={cn(
        "group relative z-10 shrink-0 touch-none outline-none",
        dir === "row" ? "-mx-1 w-2 cursor-col-resize" : "-my-1 h-2 cursor-row-resize",
        "after:absolute after:bg-stroke hover:after:bg-brand focus-visible:after:bg-brand",
        dir === "row" ? "after:inset-y-0 after:left-1/2 after:w-px after:-translate-x-1/2 hover:after:w-0.5 focus-visible:after:w-0.5" : "after:inset-x-0 after:top-1/2 after:h-px after:-translate-y-1/2 hover:after:h-0.5 focus-visible:after:h-0.5",
      )}
    />
  );
}
