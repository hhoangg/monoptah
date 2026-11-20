// @vitest-environment node
import { readFileSync } from "node:fs";
import path from "node:path";
import { describe, expect, it } from "vitest";
import { EXPECTED_STYLE_CALLS, replaceStyleElements, xtermStaticStyles } from "./xtermStaticStyles.ts";

const id = path.resolve(import.meta.dirname, "../node_modules/@xterm/xterm/lib/xterm.mjs");

describe("xtermStaticStyles", () => {
  it("rewrites the three runtime <style> calls of the installed xterm bundle and leaves no createElement(style) behind", () => {
    const code = readFileSync(id, "utf8");
    expect(code.match(/createElement\("style"\)/g)?.length).toBe(EXPECTED_STYLE_CALLS);
    const result = replaceStyleElements(code, id)!;
    expect(result.match(/__xtermSheetStyle\(\)/g)?.length).toBe(EXPECTED_STYLE_CALLS);
    expect(result).not.toContain('createElement("style")');
    expect(result.startsWith('import { createSheetStyle as __xtermSheetStyle } from "')).toBe(true);
    expect(result).toContain("src/features/terminal/sheetStyle.ts");
  });

  it("ignores other modules, also the UMD build", () => {
    expect(replaceStyleElements('x.createElement("style")', "/x/node_modules/react/index.js")).toBeUndefined();
    expect(replaceStyleElements('x.createElement("style")', "/x/node_modules/@xterm/xterm/lib/xterm.js")).toBeUndefined();
  });

  it("throws when an xterm update changes the number of runtime <style> calls, so the build fails instead of shipping unstyled output", () => {
    expect(() => replaceStyleElements("export {}", "/x/node_modules/@xterm/xterm/lib/xterm.mjs")).toThrow(/expected 3 runtime <style> calls/);
    const two = 'this._document.createElement("style");this._document.createElement("style");';
    expect(() => replaceStyleElements(two, "/x/node_modules/@xterm/xterm/lib/xterm.mjs")).toThrow(/found 2/);
  });

  it("fails the bundle when the xterm chunk is missing or still builds a <style> element", () => {
    const plugin = xtermStaticStyles();
    const transform = plugin.transform as unknown as (this: unknown, code: string, id: string) => unknown;
    const generate = plugin.generateBundle as unknown as (this: unknown, options: unknown, bundle: Record<string, unknown>) => void;
    // Nothing transformed: nothing to check.
    expect(() => generate.call({}, {}, {})).not.toThrow();
    transform.call({}, readFileSync(id, "utf8"), id);
    expect(() => generate.call({}, {}, { "a.js": { type: "chunk", code: "export {}" } })).toThrow(/chunk was not found/);
    const fallback = 'const f=()=>document.createElement("style");const o="xterm-dom-renderer-owner";';
    expect(() => generate.call({}, {}, { "a.js": { type: "chunk", code: fallback } })).not.toThrow();
    expect(() => generate.call({}, {}, { "a.js": { type: "chunk", code: `${fallback}${fallback}` } })).toThrow(/creates 2 <style>/);
  });
});
