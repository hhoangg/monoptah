// F-47 (gap): how far the on-screen keyboard covers the bottom of the layout viewport. iOS Safari keeps the layout
// viewport at full height when the keyboard opens, so a composer pinned to the bottom would sit under it. The
// visual viewport shrinks instead; the difference is the inset the composer lifts itself by. Browsers that resize
// the layout viewport for the keyboard (`interactive-widget=resizes-content`) report 0 here.
import { useEffect, useState } from "react";

/** Below this the difference is browser chrome (a collapsing address bar), not a keyboard. */
const KEYBOARD_MIN_PX = 120;

/** Pure part: the inset for a visual viewport against the layout height; 0 while pinch-zoomed or small. */
export function keyboardInset(layoutHeight: number, viewport: { height: number; offsetTop: number; scale: number }): number {
  if (viewport.scale > 1.01) return 0;
  const covered = Math.round(layoutHeight - viewport.height - viewport.offsetTop);
  return covered >= KEYBOARD_MIN_PX ? covered : 0;
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
      setInset(typing ? keyboardInset(window.innerHeight, viewport) : 0);
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
