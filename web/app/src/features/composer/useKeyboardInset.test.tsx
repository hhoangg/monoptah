import { act, renderHook } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { keyboardLift, layoutBottom, textFieldFocused, useKeyboardInset } from "./useKeyboardInset";

describe("keyboardLift", () => {
  const vv = (height: number, offsetTop: number, pageTop: number, scale = 1) => ({ height, offsetTop, pageTop, scale });
  it("a frame that ends below the visual viewport bottom is lifted by exactly the difference, even when it is small", () => {
    // iOS scrolled the document by 365 for the keyboard; the frame spans -365..531 in client coordinates, the visual viewport ends at 483.
    expect(keyboardLift({ frameBottom: 531, clientHeight: 848, viewport: vv(483, 365, 365), scrollY: 365 })).toBe(48);
  });
  it("in-flow frame in a home-screen app (default status bar): the frame bottom is the visual viewport bottom, no lift", () => {
    expect(keyboardLift({ frameBottom: 483, clientHeight: 848, viewport: vv(483, 365, 365), scrollY: 365 })).toBe(0);
  });
  it("Safari tab with the keyboard open: no lift, the frame ends at the visual viewport bottom", () => {
    expect(keyboardLift({ frameBottom: 403, clientHeight: 750, viewport: vv(403, 347, 347), scrollY: 347 })).toBe(0);
  });
  it("at rest (no keyboard): no lift", () => {
    expect(keyboardLift({ frameBottom: 848, clientHeight: 848, viewport: vv(848, 0, 0), scrollY: 0 })).toBe(0);
  });
  it("matches the plain offsetTop formula where pageTop = scrollY + offsetTop (Chromium)", () => {
    expect(keyboardLift({ frameBottom: 800, clientHeight: 800, viewport: vv(480, 0, 0), scrollY: 0 })).toBe(320);
    expect(keyboardLift({ frameBottom: 800, clientHeight: 800, viewport: vv(460, 20, 20), scrollY: 0 })).toBe(320);
  });
  it("is 0 while pinch-zoomed, and for browser chrome that is shorter than a keyboard", () => {
    expect(keyboardLift({ frameBottom: 800, clientHeight: 800, viewport: vv(400, 0, 0, 2), scrollY: 0 })).toBe(0);
    expect(keyboardLift({ frameBottom: 800, clientHeight: 800, viewport: vv(750, 0, 0), scrollY: 0 })).toBe(0);
  });
  it("never lifts by a negative amount", () => {
    expect(keyboardLift({ frameBottom: 400, clientHeight: 800, viewport: vv(500, 0, 0), scrollY: 0 })).toBe(0);
  });
});

describe("useKeyboardInset", () => {
  afterEach(() => {
    Object.defineProperty(window, "visualViewport", { value: null, configurable: true });
    document.body.replaceChildren();
  });
  const field = () => {
    const textarea = document.createElement("textarea");
    document.body.append(textarea);
    return textarea;
  };
  it("follows the visual viewport while a text field has focus", () => {
    const viewport = Object.assign(new EventTarget(), { height: 800, offsetTop: 0, scale: 1 });
    Object.defineProperty(window, "visualViewport", { value: viewport, configurable: true });
    Object.defineProperty(window, "innerHeight", { value: 800, configurable: true });
    const { result } = renderHook(() => useKeyboardInset());
    expect(result.current).toBe(0);
    act(() => field().focus());
    act(() => {
      viewport.height = 500;
      viewport.dispatchEvent(new Event("resize"));
    });
    expect(result.current).toBe(300);
    act(() => {
      viewport.height = 800;
      viewport.dispatchEvent(new Event("resize"));
    });
    expect(result.current).toBe(0);
  });
  it("reads a short visual viewport as no keyboard while nothing is focused (no floating gap)", () => {
    const viewport = Object.assign(new EventTarget(), { height: 600, offsetTop: 0, scale: 1 });
    Object.defineProperty(window, "visualViewport", { value: viewport, configurable: true });
    Object.defineProperty(window, "innerHeight", { value: 800, configurable: true });
    const { result } = renderHook(() => useKeyboardInset());
    expect(result.current).toBe(0);
    act(() => viewport.dispatchEvent(new Event("resize")));
    expect(result.current).toBe(0);
  });
  it("drops the inset when the field loses focus, even if the viewport stays short", async () => {
    const viewport = Object.assign(new EventTarget(), { height: 500, offsetTop: 0, scale: 1 });
    Object.defineProperty(window, "visualViewport", { value: viewport, configurable: true });
    Object.defineProperty(window, "innerHeight", { value: 800, configurable: true });
    const input = field();
    input.focus();
    const { result } = renderHook(() => useKeyboardInset());
    expect(result.current).toBe(300);
    await act(async () => {
      input.blur();
      await new Promise((resolve) => setTimeout(resolve, 5));
    });
    expect(result.current).toBe(0);
  });
});

describe("useKeyboardInset: the document is put back at the top", () => {
  afterEach(() => {
    Object.defineProperty(window, "visualViewport", { value: null, configurable: true });
    vi.restoreAllMocks();
    document.body.replaceChildren();
  });
  const stub = (viewport: { height: number; offsetTop: number; scale: number }) => {
    const target = Object.assign(new EventTarget(), viewport);
    Object.defineProperty(window, "visualViewport", { value: target, configurable: true });
    Object.defineProperty(window, "innerHeight", { value: 800, configurable: true });
    return target;
  };
  it("scrolls to 0 on a visual viewport resize when it is panned and no field is focused", () => {
    const scrollTo = vi.spyOn(window, "scrollTo").mockImplementation(() => {});
    const viewport = stub({ height: 800, offsetTop: 0, scale: 1 });
    renderHook(() => useKeyboardInset());
    expect(scrollTo).not.toHaveBeenCalled();
    act(() => {
      viewport.offsetTop = 40;
      viewport.dispatchEvent(new Event("resize"));
    });
    expect(scrollTo).toHaveBeenCalledWith(0, 0);
  });
  it("also does it on a visual viewport scroll, and when the document itself was scrolled", () => {
    const scrollTo = vi.spyOn(window, "scrollTo").mockImplementation(() => {});
    const viewport = stub({ height: 800, offsetTop: 0, scale: 1 });
    renderHook(() => useKeyboardInset());
    // jsdom has no scrollingElement: a scrolled document is simulated.
    Object.defineProperty(document, "scrollingElement", { value: { scrollTop: 60 }, configurable: true });
    try {
      act(() => viewport.dispatchEvent(new Event("scroll")));
      expect(scrollTo).toHaveBeenCalledWith(0, 0);
    } finally {
      Reflect.deleteProperty(document, "scrollingElement");
    }
  });
  it("does not do it while a field is focused (the keyboard is up) or while pinch-zoomed", () => {
    const scrollTo = vi.spyOn(window, "scrollTo").mockImplementation(() => {});
    const viewport = stub({ height: 500, offsetTop: 40, scale: 1 });
    const textarea = document.createElement("textarea");
    document.body.append(textarea);
    textarea.focus();
    renderHook(() => useKeyboardInset());
    act(() => viewport.dispatchEvent(new Event("resize")));
    expect(scrollTo).not.toHaveBeenCalled();
    textarea.blur();
    scrollTo.mockClear();
    act(() => {
      viewport.scale = 2;
      viewport.dispatchEvent(new Event("resize"));
    });
    expect(scrollTo).not.toHaveBeenCalled();
  });
  it("reads again after the keyboard animation and clears its timers on unmount", async () => {
    vi.useFakeTimers();
    try {
      const scrollTo = vi.spyOn(window, "scrollTo").mockImplementation(() => {});
      const viewport = stub({ height: 500, offsetTop: 0, scale: 1 });
      const textarea = document.createElement("textarea");
      document.body.append(textarea);
      textarea.focus();
      const { unmount } = renderHook(() => useKeyboardInset());
      act(() => textarea.blur());
      // The visual viewport is only restored (panned back to a stale offset) after the animation.
      viewport.height = 800;
      viewport.offsetTop = 30;
      act(() => {
        vi.advanceTimersByTime(400);
      });
      expect(scrollTo).toHaveBeenCalledWith(0, 0);
      scrollTo.mockClear();
      const clear = vi.spyOn(window, "clearTimeout");
      act(() => textarea.focus());
      act(() => textarea.blur());
      unmount();
      expect(clear).toHaveBeenCalled();
      act(() => {
        vi.advanceTimersByTime(1000);
      });
      expect(scrollTo).not.toHaveBeenCalled();
    } finally {
      vi.useRealTimers();
    }
  });
});

describe("textFieldFocused", () => {
  afterEach(() => document.body.replaceChildren());
  it("is true for text fields and false for buttons and checkboxes", () => {
    const text = document.createElement("input");
    const check = document.createElement("input");
    check.type = "checkbox";
    const button = document.createElement("button");
    document.body.append(text, check, button);
    text.focus();
    expect(textFieldFocused()).toBe(true);
    check.focus();
    expect(textFieldFocused()).toBe(false);
    button.focus();
    expect(textFieldFocused()).toBe(false);
  });
});

describe("useKeyboardInset: the layout height is the bottom of the app frame", () => {
  afterEach(() => {
    Object.defineProperty(window, "visualViewport", { value: null, configurable: true });
    document.body.replaceChildren();
  });
  const stub = (height: number) => {
    const viewport = Object.assign(new EventTarget(), { height, offsetTop: 0, scale: 1 });
    Object.defineProperty(window, "visualViewport", { value: viewport, configurable: true });
    Object.defineProperty(window, "innerHeight", { value: 800, configurable: true });
    return viewport;
  };
  const frame = (bottom: number) => {
    const el = document.createElement("div");
    el.setAttribute("data-app-frame", "");
    el.getBoundingClientRect = () => ({ bottom }) as DOMRect;
    document.body.append(el);
    return el;
  };
  it("measures the keyboard against the frame bottom, not window.innerHeight", () => {
    expect(layoutBottom()).toBe(800);
    frame(780);
    expect(layoutBottom()).toBe(780);
    const viewport = stub(500);
    const input = document.createElement("textarea");
    document.body.append(input);
    input.focus();
    const { result } = renderHook(() => useKeyboardInset());
    // 780 - 500, not 800 - 500.
    expect(result.current).toBe(280);
    act(() => {
      viewport.height = 800;
      viewport.dispatchEvent(new Event("resize"));
    });
    expect(result.current).toBe(0);
  });
  it("falls back to window.innerHeight when there is no frame", () => {
    stub(500);
    const input = document.createElement("textarea");
    document.body.append(input);
    input.focus();
    const { result } = renderHook(() => useKeyboardInset());
    expect(result.current).toBe(300);
  });
});
