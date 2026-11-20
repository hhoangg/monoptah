// A long press on touch or pen (iOS fires no `contextmenu` on one, and Android fires it with the native menu): a timer
// starts on pointerdown and fires `onLongPress` after `delay` ms unless the finger moves more than `moveTolerance`
// px, lifts, the pointer is cancelled (the browser took the gesture, for example to scroll) or the page scrolls. The
// click that follows the release of a long press is swallowed, so a row that is a link does not open. Mouse input is
// ignored: it has a right click. Android Chrome also fires `contextmenu` on a long press, with its native menu: the
// caller handles that event itself (it knows whether to open its own menu there).
//
// Spread `bind` on the element. The element should also be `select-none` with `-webkit-touch-callout: none` (see
// `LONG_PRESS_SURFACE`) so iOS shows no text selection or link callout while the finger is down.
import { useCallback, useEffect, useMemo, useRef, type MouseEvent as ReactMouseEvent, type PointerEvent as ReactPointerEvent } from "react";
import { haptic } from "@/lib/haptics";

export const LONG_PRESS_DELAY_MS = 500;
export const LONG_PRESS_MOVE_TOLERANCE_PX = 8;

/** Classes that stop the iOS text selection and the link callout during a long press. */
export const LONG_PRESS_SURFACE = "select-none [-webkit-touch-callout:none]";

export interface LongPressEvent {
  /** Viewport coordinates of the finger when the press started. */
  x: number;
  y: number;
  /** The element under the finger when the press started. */
  target: EventTarget | null;
}

export interface UseLongPressOptions {
  /** Return false when the press is not yours to take (nothing happened): no haptic tick, the click is not swallowed. */
  onLongPress: (event: LongPressEvent) => boolean | void;
  /** Default 500 ms. */
  delay?: number;
  /** Default 8 px. */
  moveTolerance?: number;
  /** False turns the hook off (a mouse layout): no timer, no swallowed click, the native menu stays. */
  enabled?: boolean;
}

export interface LongPressBind {
  onPointerDown: (event: ReactPointerEvent<HTMLElement>) => void;
  onPointerMove: (event: ReactPointerEvent<HTMLElement>) => void;
  onPointerUp: (event: ReactPointerEvent<HTMLElement>) => void;
  onPointerCancel: (event: ReactPointerEvent<HTMLElement>) => void;
  onClickCapture: (event: ReactMouseEvent<HTMLElement>) => void;
}

export function useLongPress({ onLongPress, delay = LONG_PRESS_DELAY_MS, moveTolerance = LONG_PRESS_MOVE_TOLERANCE_PX, enabled = true }: UseLongPressOptions): { bind: LongPressBind; cancel: () => void } {
  const timer = useRef<ReturnType<typeof setTimeout> | null>(null);
  const start = useRef<{ id: number; x: number; y: number } | null>(null);
  // True from the moment a long press fired until the click of its release is swallowed (or the next press starts).
  const fired = useRef(false);
  const firedReset = useRef<ReturnType<typeof setTimeout> | null>(null);
  // The click of the release comes right after pointerup; if none does, the flag must not hit a later, unrelated click.
  const expireFired = useCallback(() => {
    if (firedReset.current !== null) clearTimeout(firedReset.current);
    firedReset.current = fired.current ? setTimeout(() => (fired.current = false), 100) : null;
  }, []);
  const latest = useRef(onLongPress);
  useEffect(() => {
    latest.current = onLongPress;
  });
  // The page scrolling (any scroller, hence the capture) ends the press; the listener must be one stable function.
  const cancelRef = useRef<() => void>(() => undefined);
  const onScroll = useCallback(() => cancelRef.current(), []);

  const cancel = useCallback(() => {
    if (timer.current !== null) clearTimeout(timer.current);
    timer.current = null;
    start.current = null;
    window.removeEventListener("scroll", onScroll, true);
  }, [onScroll]);

  useEffect(() => {
    cancelRef.current = cancel;
    return () => {
      cancel();
      if (firedReset.current !== null) clearTimeout(firedReset.current);
    };
  }, [cancel]);

  const bind = useMemo<LongPressBind>(
    () => ({
      onPointerDown: (event) => {
        if (firedReset.current !== null) clearTimeout(firedReset.current);
        fired.current = false;
        cancel();
        if (!enabled || event.pointerType === "mouse" || event.button !== 0 || !event.isPrimary) return;
        const { clientX: x, clientY: y, pointerId: id, target } = event;
        start.current = { id, x, y };
        window.addEventListener("scroll", onScroll, true);
        timer.current = setTimeout(() => {
          timer.current = null;
          start.current = null;
          window.removeEventListener("scroll", onScroll, true);
          if (latest.current({ x, y, target }) === false) return;
          fired.current = true;
          haptic("medium");
        }, delay);
      },
      onPointerMove: (event) => {
        const origin = start.current;
        if (!origin || origin.id !== event.pointerId) return;
        if (Math.hypot(event.clientX - origin.x, event.clientY - origin.y) > moveTolerance) cancel();
      },
      onPointerUp: () => {
        cancel();
        expireFired();
      },
      onPointerCancel: () => {
        cancel();
        expireFired();
      },
      onClickCapture: (event) => {
        if (!fired.current) return;
        fired.current = false;
        event.preventDefault();
        event.stopPropagation();
      },
    }),
    [cancel, delay, enabled, expireFired, moveTolerance, onScroll],
  );

  return { bind, cancel };
}
