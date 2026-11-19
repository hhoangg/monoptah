// F-47 (gap): how far the on-screen keyboard covers the bottom of the layout viewport. iOS Safari keeps the layout
// viewport at full height when the keyboard opens, so a composer pinned to the bottom would sit under it. The
// visual viewport shrinks instead; the difference is the inset the composer lifts itself by. The layout height is the
// bottom of the /d app frame (`[data-app-frame]`), in client coordinates: that is where the composer sits, whatever the
// document scroll is (iOS scrolls the document to reveal a focused field, which moves the frame's client rect).
// `window.innerHeight` is only the fallback when there is no frame: on iOS it follows the visual viewport. Browsers
// that resize for the keyboard (`interactive-widget=resizes-content`) report 0 here.
import { useEffect, useState } from "react";

/** Below this the layout is only short by browser chrome (a collapsing address bar), not by a keyboard. */
const KEYBOARD_MIN_PX = 120;

export interface KeyboardLiftInput {
  /** Client-coordinate bottom of the app frame (what the composer is pinned to). */
  frameBottom: number;
  /** `document.documentElement.clientHeight`: the layout viewport. `innerHeight` is not used, it follows the visual viewport on iOS. */
  clientHeight: number;
  viewport: { height: number; offsetTop: number; pageTop: number; scale: number };
  scrollY: number;
}

/**
 * Pure part: how far to lift the composer above the keyboard, 0 when no keyboard is up. KEYBOARD_MIN_PX only decides
 * whether a keyboard is up (the layout viewport is that much taller than the visual one, and no pinch zoom); once it
 * is, the lift is applied as measured, even when small (a frame that ends a little below the visual viewport). The visual viewport bottom
 * is taken in client coordinates (`pageTop - scrollY + height`): on iOS the client coordinates of the frame already
 * moved with the scroll, so subtracting `offsetTop` as well would count the scroll twice.
 */
export function keyboardLift({ frameBottom, clientHeight, viewport, scrollY }: KeyboardLiftInput): number {
  if (viewport.scale > 1.01) return 0;
  if (clientHeight - viewport.height < KEYBOARD_MIN_PX) return 0;
  const visualBottom = viewport.pageTop - scrollY + viewport.height;
  return Math.max(0, Math.round(frameBottom - visualBottom));
}

/** Reads the live browser state into `keyboardLift`. `pageTop` falls back to `scrollY + offsetTop` where it is missing. */
export function readKeyboardLift(viewport: VisualViewport): number {
  const scrollY = window.scrollY;
  return keyboardLift({
    frameBottom: layoutBottom(),
    clientHeight: document.documentElement.clientHeight || window.innerHeight,
    viewport: { height: viewport.height, offsetTop: viewport.offsetTop, pageTop: viewport.pageTop ?? scrollY + viewport.offsetTop, scale: viewport.scale },
    scrollY,
  });
}

/** The layout height the composer is pinned to: the bottom of the /d app frame, else `window.innerHeight`. */
export function layoutBottom(): number {
  const frame = document.querySelector("[data-app-frame]");
  return frame ? frame.getBoundingClientRect().bottom : window.innerHeight;
}

/** True while a text field has focus: the only time an on-screen keyboard can be up. */
export function textFieldFocused(): boolean {
  const el = document.activeElement;
  if (!(el instanceof HTMLElement)) return false;
  if (el.isContentEditable) return true;
  if (el instanceof HTMLTextAreaElement) return !el.readOnly && !el.disabled;
  return el instanceof HTMLInputElement && !el.readOnly && !el.disabled && !/^(button|checkbox|radio|submit|reset|range|file|color|image)$/.test(el.type);
}

/** How long after a field loses focus the keyboard may still be animating away; the state is read again then. */
const BLUR_SETTLE_MS = 350;

/**
 * Two guards against a composer that floats above the bottom edge with an empty gap, both for iOS:
 *
 * 1. The inset is only trusted while a text field has focus. A visual viewport that is short for another reason (a
 *    stale height after the keyboard closes in a standalone app, or browser chrome taller than KEYBOARD_MIN_PX) would
 *    otherwise read as a keyboard, lift the composer and drop the safe-area padding.
 * 2. The /d shell is a fixed-height frame, so the document never scrolls by design. iOS scrolls it (and pans the visual
 *    viewport) to reveal a focused field and can leave it there after the keyboard closes. With no field focused, on
 *    every visual viewport resize and scroll (and again after the keyboard animation), the document is put back at 0.
 */
export function useKeyboardInset(): number {
  const [inset, setInset] = useState(0);
  useEffect(() => {
    const viewport = window.visualViewport;
    if (!viewport) return;
    let timers: number[] = [];
    const update = () => {
      const typing = textFieldFocused();
      if (!typing && viewport.scale <= 1.01) {
        const scroller = document.scrollingElement;
        if ((scroller && scroller.scrollTop !== 0) || viewport.offsetTop > 0) window.scrollTo(0, 0);
      }
      setInset(typing ? readKeyboardLift(viewport) : 0);
    };
    const afterBlur = () => {
      timers.forEach((timer) => window.clearTimeout(timer));
      timers = [window.setTimeout(update, 0), window.setTimeout(update, BLUR_SETTLE_MS)];
    };
    update();
    viewport.addEventListener("resize", update);
    viewport.addEventListener("scroll", update);
    document.addEventListener("focusin", update);
    document.addEventListener("focusout", afterBlur);
    return () => {
      timers.forEach((timer) => window.clearTimeout(timer));
      viewport.removeEventListener("resize", update);
      viewport.removeEventListener("scroll", update);
      document.removeEventListener("focusin", update);
      document.removeEventListener("focusout", afterBlur);
    };
  }, []);
  return inset;
}
