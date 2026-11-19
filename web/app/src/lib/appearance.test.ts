import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { applyAppearance, resolveScheme, startAppearanceSync, useAppearance } from "./appearance";

function mockMedia(dark: boolean) {
  const listeners = new Set<() => void>();
  const state = { dark };
  window.matchMedia = ((query: string) => ({
    matches: query.includes("dark") ? state.dark : false,
    media: query,
    addEventListener: (_: string, fn: () => void) => listeners.add(fn),
    removeEventListener: (_: string, fn: () => void) => listeners.delete(fn),
  })) as unknown as typeof window.matchMedia;
  return {
    set(next: boolean) {
      state.dark = next;
      listeners.forEach((fn) => fn());
    },
  };
}

const root = () => document.documentElement;

beforeEach(() => {
  localStorage.clear();
  root().removeAttribute("data-scheme");
  root().removeAttribute("data-theme");
  root().removeAttribute("data-accent");
  document.head.replaceChildren();
  const meta = document.createElement("meta");
  meta.setAttribute("name", "theme-color");
  meta.setAttribute("content", "#131313");
  document.head.append(meta);
});
afterEach(() => vi.restoreAllMocks());

describe("resolveScheme", () => {
  it("resolves system through the media query result", () => {
    expect(resolveScheme("system", true)).toBe("dark");
    expect(resolveScheme("system", false)).toBe("light");
    expect(resolveScheme("light", true)).toBe("light");
    expect(resolveScheme("dark", false)).toBe("dark");
  });
});

describe("appearance store", () => {
  it("persists under monoptah.* keys and writes the attributes and theme-color", () => {
    mockMedia(true);
    const s = useAppearance.getState();
    s.setAppearance("light");
    s.setTheme("ocean");
    s.setAccent("teal");
    expect(localStorage.getItem("monoptah.appearance")).toBe("light");
    expect(localStorage.getItem("monoptah.theme")).toBe("ocean");
    expect(localStorage.getItem("monoptah.accent")).toBe("teal");
    expect(root()).toHaveAttribute("data-scheme", "light");
    expect(root()).toHaveAttribute("data-theme", "ocean");
    expect(root()).toHaveAttribute("data-accent", "teal");
    expect(document.querySelector('meta[name="theme-color"]')).toHaveAttribute("content", "#f3f6f9");
    expect(useAppearance.getState().scheme).toBe("light");
  });

  it("applies the dark background of the theme to theme-color", () => {
    mockMedia(false);
    useAppearance.getState().setTheme("rose");
    useAppearance.getState().setAppearance("dark");
    expect(document.querySelector('meta[name="theme-color"]')).toHaveAttribute("content", "#160e11");
  });

  it("follows the media query in system mode and the storage event across tabs", () => {
    const media = mockMedia(false);
    useAppearance.getState().setAppearance("system");
    const stop = startAppearanceSync();
    expect(root()).toHaveAttribute("data-scheme", "light");
    media.set(true);
    expect(root()).toHaveAttribute("data-scheme", "dark");

    // Another tab changed the theme.
    localStorage.setItem("monoptah.theme", "grove");
    window.dispatchEvent(new StorageEvent("storage", { key: "monoptah.theme", newValue: "grove" }));
    expect(useAppearance.getState().theme).toBe("grove");
    expect(root()).toHaveAttribute("data-theme", "grove");

    // Other keys are ignored.
    const before = useAppearance.getState();
    window.dispatchEvent(new StorageEvent("storage", { key: "unrelated", newValue: "x" }));
    expect(useAppearance.getState()).toBe(before);

    stop();
    media.set(false);
    expect(root()).toHaveAttribute("data-scheme", "dark"); // listener removed
  });

  it("ignores stored values outside the allowed lists", () => {
    mockMedia(true);
    localStorage.setItem("monoptah.theme", "neon");
    localStorage.setItem("monoptah.accent", "<script>");
    useAppearance.getState().sync();
    expect(useAppearance.getState().theme).toBe("mono");
    expect(useAppearance.getState().accent).toBe("theme");
  });

  it("applyAppearance returns the applied scheme", () => {
    mockMedia(false);
    expect(applyAppearance({ appearance: "system", theme: "mono", accent: "theme" })).toBe("light");
  });
});
