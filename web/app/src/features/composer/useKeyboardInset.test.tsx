import { act, renderHook } from "@testing-library/react";
import { afterEach, describe, expect, it } from "vitest";
import { keyboardInset, useKeyboardInset } from "./useKeyboardInset";

describe("keyboardInset", () => {
  it("is the part of the layout the visual viewport no longer covers", () => {
    expect(keyboardInset(800, { height: 480, offsetTop: 0, scale: 1 })).toBe(320);
    expect(keyboardInset(800, { height: 460, offsetTop: 20, scale: 1 })).toBe(320);
  });
  it("ignores browser chrome, and pinch zoom", () => {
    expect(keyboardInset(800, { height: 750, offsetTop: 0, scale: 1 })).toBe(0);
    expect(keyboardInset(800, { height: 400, offsetTop: 0, scale: 2 })).toBe(0);
  });
});

describe("useKeyboardInset", () => {
  afterEach(() => {
    Object.defineProperty(window, "visualViewport", { value: null, configurable: true });
  });
  it("follows the visual viewport", () => {
    const viewport = Object.assign(new EventTarget(), { height: 800, offsetTop: 0, scale: 1 });
    Object.defineProperty(window, "visualViewport", { value: viewport, configurable: true });
    Object.defineProperty(window, "innerHeight", { value: 800, configurable: true });
    const { result } = renderHook(() => useKeyboardInset());
    expect(result.current).toBe(0);
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
});
