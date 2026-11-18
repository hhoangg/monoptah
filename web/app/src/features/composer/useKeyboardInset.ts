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

export function useKeyboardInset(): number {
  const [inset, setInset] = useState(0);
  useEffect(() => {
    const viewport = window.visualViewport;
    if (!viewport) return;
    const update = () => setInset(keyboardInset(window.innerHeight, viewport));
    update();
    viewport.addEventListener("resize", update);
    viewport.addEventListener("scroll", update);
    return () => {
      viewport.removeEventListener("resize", update);
      viewport.removeEventListener("scroll", update);
    };
  }, []);
  return inset;
}
