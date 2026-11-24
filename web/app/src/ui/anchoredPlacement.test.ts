import { afterEach, describe, expect, it, vi } from "vitest";
import { placeAnchored, readVisibleBounds, revealScrollTop } from "./anchoredPlacement";

const base = { anchorTop: 600, anchorBottom: 700, boundsTop: 8, boundsBottom: 800, gap: 8, cap: 288, minHeight: 120 };

describe("placeAnchored", () => {
  it("goes above and applies the cap when there is plenty of room above", () => {
    expect(placeAnchored(base)).toEqual({ side: "above", maxHeight: 288 });
  });

  it("takes the room above when the keyboard leaves less than the cap", () => {
    // Top bar bottom at 68, composer top at 330: 330 - 8 - 68 = 254.
    expect(placeAnchored({ ...base, anchorTop: 330, anchorBottom: 480, boundsTop: 68, boundsBottom: 492 })).toEqual({ side: "above", maxHeight: 254 });
  });

  it("flips below when above is under the minimum and below has more room", () => {
    expect(placeAnchored({ ...base, anchorTop: 60, anchorBottom: 110 })).toEqual({ side: "below", maxHeight: 288 });
    expect(placeAnchored({ ...base, anchorTop: 60, anchorBottom: 560 })).toEqual({ side: "below", maxHeight: 232 });
  });

  it("stays above when above is small but below is smaller", () => {
    expect(placeAnchored({ ...base, anchorTop: 100, anchorBottom: 790 })).toEqual({ side: "above", maxHeight: 84 });
  });

  it("decides the side on the visible room but sizes from the whole room on that side", () => {
    // Anchor top 340 under the visible bottom 300: it stays above (visible room 232), and the height is 340 - 8 - 68 = 264.
    expect(placeAnchored({ ...base, anchorTop: 340, anchorBottom: 440, boundsTop: 68, boundsBottom: 300 })).toEqual({ side: "above", maxHeight: 264 });
    expect(placeAnchored({ ...base, anchorTop: 900, anchorBottom: 1000, boundsTop: 8, boundsBottom: 200 })).toEqual({ side: "above", maxHeight: 288 });
    // Past the visible bottom there is no visible room below, so it does not flip even when above is small.
    expect(placeAnchored({ ...base, anchorTop: 900, anchorBottom: 1000, boundsTop: 8, boundsBottom: 100 })).toEqual({ side: "above", maxHeight: 288 });
    // An anchor above the visible top has no visible room above: it flips below, sized from the room under the anchor.
    expect(placeAnchored({ ...base, anchorTop: -300, anchorBottom: -200, boundsTop: 8, boundsBottom: 300 })).toEqual({ side: "below", maxHeight: 288 });
    expect(placeAnchored({ ...base, anchorTop: -300, anchorBottom: -200, boundsTop: 8, boundsBottom: 100, cap: 1000 })).toEqual({ side: "below", maxHeight: 292 });
  });

  it("never returns a negative height", () => {
    expect(placeAnchored({ ...base, anchorTop: 4, anchorBottom: 800 })).toEqual({ side: "above", maxHeight: 0 });
  });

  it("keeps a cap that is smaller than the room", () => {
    expect(placeAnchored({ ...base, cap: 100 })).toEqual({ side: "above", maxHeight: 100 });
  });
});

describe("readVisibleBounds", () => {
  const original = Object.getOwnPropertyDescriptor(window, "visualViewport");
  afterEach(() => {
    if (original) Object.defineProperty(window, "visualViewport", original);
    else Reflect.deleteProperty(window, "visualViewport");
    document.body.replaceChildren();
  });

  function mount() {
    const main = document.createElement("main");
    main.style.overflowY = "auto";
    const anchor = document.createElement("div");
    main.append(anchor);
    document.body.replaceChildren(main);
    vi.spyOn(main, "getBoundingClientRect").mockReturnValue({ top: 60, bottom: 844, left: 0, right: 390, width: 390, height: 784, x: 0, y: 60, toJSON: () => ({}) });
    return anchor;
  }

  it("cuts the short visual viewport down to the clipping ancestor and keeps a margin", () => {
    Object.defineProperty(window, "visualViewport", { configurable: true, value: { pageTop: 0, offsetTop: 0, height: 500 } });
    expect(readVisibleBounds(mount())).toEqual({ top: 68, bottom: 492, viewportHeight: 500 });
  });

  it("reads the visual viewport in client coordinates when the document is scrolled", () => {
    Object.defineProperty(window, "visualViewport", { configurable: true, value: { pageTop: 50, offsetTop: 50, height: 500 } });
    Object.defineProperty(window, "scrollY", { configurable: true, value: 30 });
    try {
      expect(readVisibleBounds(mount())).toEqual({ top: 68, bottom: 512, viewportHeight: 500 });
    } finally {
      Object.defineProperty(window, "scrollY", { configurable: true, value: 0 });
    }
  });

  it("ignores a pinch-zoomed visual viewport", () => {
    Object.defineProperty(window, "visualViewport", { configurable: true, value: { pageTop: 0, offsetTop: 0, height: 300, scale: 2 } });
    const height = document.documentElement.clientHeight || window.innerHeight;
    expect(readVisibleBounds(mount())).toEqual({ top: 68, bottom: Math.min(height, 844) - 8, viewportHeight: height });
  });

  it("falls back to the layout viewport without a visualViewport", () => {
    Object.defineProperty(window, "visualViewport", { configurable: true, value: undefined });
    const bounds = readVisibleBounds(mount());
    const height = document.documentElement.clientHeight || window.innerHeight;
    expect(bounds).toEqual({ top: 68, bottom: Math.min(height, 844) - 8, viewportHeight: height });
  });
});

describe("revealScrollTop", () => {
  const view = { viewHeight: 100, padTop: 4, padBottom: 4 };
  it("scrolls up to an item above the view, keeping the padding", () => {
    expect(revealScrollTop({ ...view, scrollTop: 100, itemTop: 60, itemBottom: 100 })).toBe(56);
    expect(revealScrollTop({ ...view, scrollTop: 100, itemTop: 2, itemBottom: 40 })).toBe(0);
  });
  it("scrolls down to an item below the view, keeping the padding", () => {
    expect(revealScrollTop({ ...view, scrollTop: 0, itemTop: 90, itemBottom: 130 })).toBe(34);
  });
  it("leaves an item that is already visible alone", () => {
    expect(revealScrollTop({ ...view, scrollTop: 20, itemTop: 40, itemBottom: 80 })).toBe(20);
  });
});
