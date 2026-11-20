// iOS-style swipe actions of a session row: a touch or pen drag from right to left slides the row left and reveals the
// buttons behind it; letting go past half the width (or with a quick flick) snaps it open, otherwise it snaps shut.
// Only one row is open at a time (a module level store), a tap anywhere else closes it, and so does a tap on the row.
//
// Direction lock: the row has `touch-action: pan-y`, so the browser keeps vertical scrolling. The first 10 px of a
// gesture decide it: mostly vertical hands the gesture to the browser (it sends `pointercancel`), mostly horizontal
// is ours. Mouse input is ignored (a desktop layout has the menu button).
import { useCallback, useEffect, useRef, useState, useSyncExternalStore, type MouseEvent as ReactMouseEvent, type PointerEvent as ReactPointerEvent, type RefObject } from "react";

/** Movement before the gesture picks its direction. */
export const SWIPE_LOCK_PX = 10;
/** A flick faster than this (px per ms) decides the snap on its own. */
const FLICK_SPEED = 0.5;

let openId: string | null = null;
const listeners = new Set<() => void>();

function setOpenId(next: string | null): void {
  if (openId === next) return;
  openId = next;
  listeners.forEach((listener) => listener());
}

function subscribe(listener: () => void): () => void {
  listeners.add(listener);
  return () => listeners.delete(listener);
}

/** Closes whichever row is open (tests reset with it). */
export function closeOpenSwipeRow(): void {
  setOpenId(null);
}

export interface SwipeBind {
  onPointerDown: (event: ReactPointerEvent<HTMLElement>) => void;
  onPointerMove: (event: ReactPointerEvent<HTMLElement>) => void;
  onPointerUp: (event: ReactPointerEvent<HTMLElement>) => void;
  onPointerCancel: (event: ReactPointerEvent<HTMLElement>) => void;
  onClickCapture: (event: ReactMouseEvent<HTMLElement>) => void;
}

export interface UseSwipeRevealOptions {
  /** Identifies the row in the one-open-at-a-time store. */
  id: string;
  /** False (a mouse layout, or a row with no action to offer) turns the gesture off and closes the row. */
  enabled: boolean;
  /** Width of the revealed buttons in px. */
  width: number;
  /** The row element: a pointerdown outside it closes it. */
  rowRef: RefObject<HTMLElement | null>;
}

export interface SwipeReveal {
  bind: SwipeBind;
  /** Current horizontal offset in px, 0 (shut) to -width (open). */
  offset: number;
  /** True while a finger drags the row (no transition then). */
  dragging: boolean;
  open: boolean;
  close: () => void;
}

interface Gesture {
  pointerId: number;
  x: number;
  y: number;
  base: number;
  mode: "pending" | "x";
  lastX: number;
  lastT: number;
  speed: number;
}

export function useSwipeReveal({ id, enabled, width, rowRef }: UseSwipeRevealOptions): SwipeReveal {
  const current = useSyncExternalStore(subscribe, () => openId, () => null);
  const open = enabled && current === id;
  const [drag, setDrag] = useState<number | null>(null);
  const gesture = useRef<Gesture | null>(null);
  // True after a drag, until the click of its release is swallowed (or the next press starts).
  const dragged = useRef(false);
  const draggedReset = useRef<ReturnType<typeof setTimeout> | null>(null);

  const close = useCallback(() => {
    if (openId === id) setOpenId(null);
  }, [id]);

  // A row that goes away, or stops being swipeable, does not stay "the open one".
  useEffect(() => {
    if (!enabled) close();
    return close;
  }, [enabled, close]);

  useEffect(() => () => {
    if (draggedReset.current !== null) clearTimeout(draggedReset.current);
  }, []);

  // A press anywhere outside the open row closes it (this is also how a scroll of the list starts). When it lands on a
  // link (another row), the tap only closes: its click is swallowed so nothing navigates.
  useEffect(() => {
    if (!open) return;
    const outside = (event: PointerEvent) => {
      if (event.target instanceof Node && rowRef.current?.contains(event.target)) return;
      close();
      if (!(event.target instanceof Element) || !event.target.closest("a[href]")) return;
      const swallow = (click: Event) => {
        click.preventDefault();
        click.stopPropagation();
        done();
      };
      const done = () => {
        clearTimeout(timer);
        document.removeEventListener("click", swallow, true);
      };
      const timer = setTimeout(done, 500);
      document.addEventListener("click", swallow, true);
    };
    document.addEventListener("pointerdown", outside, true);
    return () => document.removeEventListener("pointerdown", outside, true);
  }, [open, close, rowRef]);

  const bind: SwipeBind = {
    onPointerDown: (event) => {
      if (draggedReset.current !== null) clearTimeout(draggedReset.current);
      dragged.current = false;
      gesture.current = null;
      if (!enabled || event.pointerType === "mouse" || event.button !== 0 || !event.isPrimary) return;
      gesture.current = { pointerId: event.pointerId, x: event.clientX, y: event.clientY, base: open ? -width : 0, mode: "pending", lastX: event.clientX, lastT: event.timeStamp, speed: 0 };
    },
    onPointerMove: (event) => {
      const g = gesture.current;
      if (!g || g.pointerId !== event.pointerId) return;
      const dx = event.clientX - g.x;
      const dy = event.clientY - g.y;
      if (g.mode === "pending") {
        if (Math.max(Math.abs(dx), Math.abs(dy)) < SWIPE_LOCK_PX) return;
        // Mostly vertical: the browser scrolls the list, the gesture is not ours.
        if (Math.abs(dy) >= Math.abs(dx)) {
          gesture.current = null;
          return;
        }
        g.mode = "x";
        try {
          event.currentTarget.setPointerCapture(event.pointerId);
        } catch {
          // A synthetic pointer has no capture: the drag still works inside the row.
        }
      }
      const dt = event.timeStamp - g.lastT;
      if (dt > 0) g.speed = (event.clientX - g.lastX) / dt;
      g.lastX = event.clientX;
      g.lastT = event.timeStamp;
      setDrag(Math.min(0, Math.max(-width, g.base + dx)));
    },
    onPointerUp: (event) => {
      const g = gesture.current;
      gesture.current = null;
      if (!g || g.mode !== "x" || g.pointerId !== event.pointerId) return;
      dragged.current = true;
      // No click after the release (a browser may skip it): the flag must not hit a later tap.
      draggedReset.current = setTimeout(() => (dragged.current = false), 100);
      const final = Math.min(0, Math.max(-width, g.base + event.clientX - g.x));
      const next = g.speed <= -FLICK_SPEED ? true : g.speed >= FLICK_SPEED ? false : final < -width / 2;
      setDrag(null);
      if (next) setOpenId(id);
      else close();
    },
    onPointerCancel: (event) => {
      const g = gesture.current;
      gesture.current = null;
      if (g?.mode === "x" && g.pointerId === event.pointerId) setDrag(null);
    },
    onClickCapture: (event) => {
      // The buttons behind the row are not the row.
      if (event.target instanceof Element && event.target.closest("[data-swipe-actions]")) return;
      const afterDrag = dragged.current;
      dragged.current = false;
      if (!afterDrag && !open) return;
      // The release of a drag, or a tap on an open row, opens nothing; the tap also shuts the row.
      event.preventDefault();
      event.stopPropagation();
      if (!afterDrag) close();
    },
  };

  return { bind, offset: drag ?? (open ? -width : 0), dragging: drag !== null, open, close };
}
