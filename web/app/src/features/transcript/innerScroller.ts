// Copied from src/shared/hooks/useLockOverscroll.ts at ca73ac2b
// Only the nested-scroller probe is kept (the overscroll lock itself is a desktop chrome hook).

type Edges = Pick<
  HTMLElement,
  | "scrollTop"
  | "scrollLeft"
  | "clientHeight"
  | "clientWidth"
  | "scrollHeight"
  | "scrollWidth"
>;
type Delta = Pick<WheelEvent, "deltaX" | "deltaY">;

/** The scroller still has somewhere to go in the direction of the wheel. */
export function hasScrollRoom(el: Edges, e: Delta): boolean {
  if (e.deltaY < 0 && el.scrollTop > 0) return true;
  if (e.deltaY > 0 && el.scrollTop + el.clientHeight < el.scrollHeight - 1) {
    return true;
  }
  if (e.deltaX < 0 && el.scrollLeft > 0) return true;
  return e.deltaX > 0 && el.scrollLeft + el.clientWidth < el.scrollWidth - 1;
}

function scrollable(node: HTMLElement, e: Delta): boolean {
  const style = getComputedStyle(node);
  return hasScrollRoom(node, {
    deltaX: /auto|scroll|overlay/.test(style.overflowX) ? e.deltaX : 0,
    deltaY: /auto|scroll|overlay/.test(style.overflowY) ? e.deltaY : 0,
  });
}

/**
 * A scroller between the wheel's target and `el` that can still take the
 * gesture. Cancelling here would cancel it for that scroller too, since the
 * browser picks what to scroll only after the event has finished dispatching.
 */
export function innerScrollerTakes(
  el: HTMLElement,
  e: Pick<WheelEvent, "target" | "deltaX" | "deltaY">,
): boolean {
  let node = e.target instanceof Element ? e.target : null;
  while (node && node !== el) {
    // Geometry first: it is free, and it rules out most of the ancestry
    // before anything has to resolve style.
    if (
      node instanceof HTMLElement &&
      hasScrollRoom(node, e) &&
      scrollable(node, e)
    )
      return true;
    node = node.parentElement;
  }
  return false;
}
