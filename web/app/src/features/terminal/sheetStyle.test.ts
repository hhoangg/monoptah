import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { createSheetStyle } from "./sheetStyle";

// jsdom has no constructable stylesheets: a minimal stand-in shows what the stand-in element does with them.
class FakeSheet {
  css = "";
  replaceSync(css: string) {
    this.css = css;
  }
}

afterEach(() => {
  vi.unstubAllGlobals();
  Reflect.deleteProperty(document, "adoptedStyleSheets");
});

describe("createSheetStyle with adoptable stylesheets", () => {
  beforeEach(() => {
    vi.stubGlobal("CSSStyleSheet", FakeSheet);
    Object.defineProperty(document, "adoptedStyleSheets", { configurable: true, writable: true, value: [] });
  });
  const sheets = () => document.adoptedStyleSheets as unknown as FakeSheet[];

  it("is not a <style> element and puts its textContent into an adopted stylesheet", () => {
    const element = createSheetStyle();
    document.body.appendChild(element);
    expect(element.tagName).not.toBe("STYLE");
    element.textContent = ".xterm { color: red; }";
    expect(sheets().map((s) => s.css)).toEqual([".xterm { color: red; }"]);
    expect(element.textContent).toBe(".xterm { color: red; }");
    expect(document.querySelector("style")).toBeNull();
    element.textContent = ".xterm { color: blue; }";
    expect(sheets()).toHaveLength(1);
    expect(sheets()[0]?.css).toBe(".xterm { color: blue; }");
  });
  it("drops its stylesheet and leaves the DOM when removed, and only its own", () => {
    const first = createSheetStyle();
    const second = createSheetStyle();
    document.body.append(first, second);
    first.textContent = "a{}";
    second.textContent = "b{}";
    first.remove();
    expect(sheets().map((s) => s.css)).toEqual(["b{}"]);
    expect(first.isConnected).toBe(false);
    first.remove();
    second.remove();
    expect(sheets()).toHaveLength(0);
  });
});

describe("createSheetStyle without adoptable stylesheets", () => {
  it("falls back to a real <style> element (a test environment has no CSP)", () => {
    vi.stubGlobal("CSSStyleSheet", undefined);
    expect(createSheetStyle().tagName).toBe("STYLE");
  });
});
