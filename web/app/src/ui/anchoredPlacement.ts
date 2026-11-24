// Where a popup that hangs off an anchor (the composer's `/` menu, and any later mention or file picker) may go, and how
// tall it may be. The room is what is actually visible: the visual viewport (on iOS the on-screen keyboard shrinks it
// while the layout viewport, and so `dvh`, keeps the full height) intersected with every clipping ancestor of the
// anchor. On /d the top bar is drawn inside `main` (main starts at 0), and the scroller right under the bar is the chat
// pane (`features/layout/ChatPane.tsx`, the pane body of `tabs/TabbedChat.tsx` on wide screens): its top is the top
// bar's bottom, so a popup that ran up under the bar would be cut off by it.
import { useLayoutEffect, useState, type RefObject } from "react";

/** Room kept free of the edge of the visible area. */
export const VISIBLE_MARGIN_PX = 8;
/** Space between the anchor and the popup. */
export const ANCHOR_GAP_PX = 8;
/** Below this much room on the preferred side the popup flips to the other side (when that one has more). */
export const MIN_USEFUL_HEIGHT_PX = 120;

export interface PlacementInput {
  anchorTop: number;
  anchorBottom: number;
  boundsTop: number;
  boundsBottom: number;
  gap: number;
  cap: number;
  minHeight: number;
}

export interface Placement {
  side: "above" | "below";
  maxHeight: number;
}

/** Above is preferred; it flips below only when above has less than `minHeight` and below has more room. Never negative. */
export function placeAnchored({ anchorTop, anchorBottom, boundsTop, boundsBottom, gap, cap, minHeight }: PlacementInput): Placement {
  // The side is decided on the visible room: an anchor edge outside the visible area counts as being on its edge.
  const visibleAbove = Math.max(0, Math.min(anchorTop - gap, boundsBottom) - boundsTop);
  const visibleBelow = Math.max(0, boundsBottom - Math.max(anchorBottom + gap, boundsTop));
  const side = visibleAbove < minHeight && visibleBelow > visibleAbove ? "below" : "above";
  // The height is the whole room on that side: the popup stays pinned to the anchor, so a shorter one would only show less.
  const room = side === "above" ? anchorTop - gap - boundsTop : boundsBottom - anchorBottom - gap;
  return { side, maxHeight: Math.max(0, Math.floor(Math.min(cap, room))) };
}

export interface VisibleBounds {
  top: number;
  bottom: number;
  /** Height of the visual viewport, for caps given as a share of the screen. */
  viewportHeight: number;
}

/**
 * The visual viewport in client coordinates. Same convention as `useKeyboardInset`: iOS scrolls the document to reveal a
 * focused field, so the top is `pageTop - scrollY` (falling back to `offsetTop` where `pageTop` is missing). Without a
 * `visualViewport` it is the layout viewport.
 */
function visualViewportBounds(): VisibleBounds {
  const viewport = window.visualViewport;
  // Pinch zoom also shrinks the visual viewport; that is not a keyboard, so the layout viewport is used (as `keyboardLift` does).
  if (!viewport || viewport.scale > 1.01) {
    const height = document.documentElement.clientHeight || window.innerHeight;
    return { top: 0, bottom: height, viewportHeight: height };
  }
  const scrollY = window.scrollY;
  const top = (viewport.pageTop ?? scrollY + viewport.offsetTop) - scrollY;
  return { top, bottom: top + viewport.height, viewportHeight: viewport.height };
}

/** Whether `element` clips vertically. Only `overflow-y` matters: when either axis is a scroller the browser computes the other as `auto`. */
function clipsVertically(element: Element): boolean {
  return getComputedStyle(element).overflowY !== "visible";
}

/** The visible area for `anchor`: the visual viewport cut down to every clipping ancestor, minus `margin` on top and bottom. */
export function readVisibleBounds(anchor: Element, margin = VISIBLE_MARGIN_PX): VisibleBounds {
  const bounds = visualViewportBounds();
  let top = bounds.top;
  let bottom = bounds.bottom;
  for (let node = anchor.parentElement; node && node !== document.body && node !== document.documentElement; node = node.parentElement) {
    if (!clipsVertically(node)) continue;
    const rect = node.getBoundingClientRect();
    top = Math.max(top, rect.top);
    bottom = Math.min(bottom, rect.bottom);
  }
  return { top: top + margin, bottom: bottom - margin, viewportHeight: bounds.viewportHeight };
}

export interface AnchoredPlacementOptions {
  gap?: number;
  minHeight?: number;
  /** The tallest the popup may be, as pixels or as a function of the visual viewport height. A function must be stable (module level or memoized): a new one re-measures. */
  cap: number | ((viewportHeight: number) => number);
}

/**
 * Measures where `floating` (whose parent is the anchor) fits while `open`. Measured in a layout effect, so the first
 * paint already has the right size, and again on anything that moves the visible area or the anchor: visual viewport
 * resize and scroll (keyboard), window resize, a scroll of any ancestor, and a size change of the anchor (the composer
 * grows with its text). Re-measures are throttled to one per frame. `null` until measured, or when there is no layout
 * to measure (an anchor with no size).
 */
export function useAnchoredPlacement(floating: RefObject<HTMLElement | null>, open: boolean, { gap = ANCHOR_GAP_PX, minHeight = MIN_USEFUL_HEIGHT_PX, cap }: AnchoredPlacementOptions): Placement | null {
  const [placement, setPlacement] = useState<Placement | null>(null);
  useLayoutEffect(() => {
    const anchor = floating.current?.parentElement;
    if (!open || !anchor) return;
    const measure = () => {
      const rect = anchor.getBoundingClientRect();
      if (rect.width === 0 && rect.height === 0) return setPlacement(null);
      const bounds = readVisibleBounds(anchor);
      const next = placeAnchored({
        anchorTop: rect.top,
        anchorBottom: rect.bottom,
        boundsTop: bounds.top,
        boundsBottom: bounds.bottom,
        gap,
        cap: typeof cap === "function" ? cap(bounds.viewportHeight) : cap,
        minHeight,
      });
      setPlacement((prev) => (prev && prev.side === next.side && prev.maxHeight === next.maxHeight ? prev : next));
    };
    measure();
    let frame = 0;
    const schedule = () => {
      if (frame) return;
      frame = requestAnimationFrame(() => {
        frame = 0;
        measure();
      });
    };
    const viewport = window.visualViewport;
    viewport?.addEventListener("resize", schedule);
    viewport?.addEventListener("scroll", schedule);
    window.addEventListener("resize", schedule);
    // Capture phase sees every scroll in the page; only the document or an ancestor of the anchor moves it.
    const onScroll = (event: Event) => {
      const target = event.target;
      if (target === document || (target instanceof Node && target.contains(anchor))) schedule();
    };
    window.addEventListener("scroll", onScroll, true);
    const observer = typeof ResizeObserver === "undefined" ? null : new ResizeObserver(schedule);
    observer?.observe(anchor);
    return () => {
      if (frame) cancelAnimationFrame(frame);
      viewport?.removeEventListener("resize", schedule);
      viewport?.removeEventListener("scroll", schedule);
      window.removeEventListener("resize", schedule);
      window.removeEventListener("scroll", onScroll, true);
      observer?.disconnect();
    };
  }, [floating, open, gap, minHeight, cap]);
  return open ? placement : null;
}

export interface RevealInput {
  scrollTop: number;
  /** Visible height of the list (`clientHeight`). */
  viewHeight: number;
  /** The item's edges in the list's scrolling content (0 is the top of the content, padding included). */
  itemTop: number;
  itemBottom: number;
  padTop: number;
  padBottom: number;
}

/** The `scrollTop` that reveals an item with the least movement, keeping the list's padding around it. Unchanged when it is already visible. */
export function revealScrollTop({ scrollTop, viewHeight, itemTop, itemBottom, padTop, padBottom }: RevealInput): number {
  if (itemTop - padTop < scrollTop) return Math.max(0, itemTop - padTop);
  if (itemBottom + padBottom > scrollTop + viewHeight) return itemBottom + padBottom - viewHeight;
  return scrollTop;
}

/** Scrolls only `list` (never its ancestors, unlike `scrollIntoView`) so that `item` is visible. */
export function revealInList(list: HTMLElement, item: HTMLElement): void {
  const listRect = list.getBoundingClientRect();
  const itemRect = item.getBoundingClientRect();
  const style = getComputedStyle(list);
  const border = parseFloat(style.borderTopWidth) || 0;
  const next = revealScrollTop({
    scrollTop: list.scrollTop,
    viewHeight: list.clientHeight,
    itemTop: itemRect.top - listRect.top - border + list.scrollTop,
    itemBottom: itemRect.bottom - listRect.top - border + list.scrollTop,
    padTop: parseFloat(style.paddingTop) || 0,
    padBottom: parseFloat(style.paddingBottom) || 0,
  });
  if (next !== list.scrollTop) list.scrollTop = next;
}
