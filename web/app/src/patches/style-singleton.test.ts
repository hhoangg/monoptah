import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { stylesheetSingleton } from "./style-singleton";

// jsdom has no constructable stylesheets: a minimal stand-in is enough to see what the patch does.
class FakeSheet {
  css = "";
  replaceSync(css: string) {
    this.css = css;
  }
}

beforeEach(() => {
  vi.stubGlobal("CSSStyleSheet", FakeSheet);
  Object.defineProperty(document, "adoptedStyleSheets", { configurable: true, writable: true, value: [] });
});
afterEach(() => {
  vi.unstubAllGlobals();
  Reflect.deleteProperty(document, "adoptedStyleSheets");
});

describe("react-style-singleton patch (S-08 patch 2)", () => {
  it("adopts one stylesheet for the first add and drops it with the last remove, without a <style> element", () => {
    const sheet = stylesheetSingleton();
    sheet.add("body{margin:0}");
    sheet.add("body{margin:0}");
    expect((document.adoptedStyleSheets as unknown as FakeSheet[]).map((s) => s.css)).toEqual(["body{margin:0}"]);
    expect(document.head.querySelector("style")).toBeNull();
    sheet.remove();
    expect(document.adoptedStyleSheets).toHaveLength(1);
    sheet.remove();
    expect(document.adoptedStyleSheets).toHaveLength(0);
  });
});
