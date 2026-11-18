// @vitest-environment node
import { readFileSync } from "node:fs";
import path from "node:path";
import { describe, expect, it } from "vitest";
import { RUNTIME_STYLE, staticLibraryCss, stripInsertCss } from "./staticLibraryCss.ts";

function installed(library: string): { id: string; code: string } {
  const id = path.resolve(import.meta.dirname, "../node_modules", library, "dist/index.mjs");
  return { id, code: readFileSync(id, "utf8") };
}

describe("staticLibraryCss (S-08 patch 1)", () => {
  for (const library of ["sonner", "vaul"]) {
    it(`removes the runtime __insertCSS call of the installed ${library}`, () => {
      const { id, code } = installed(library);
      expect(code).toMatch(/^__insertCSS\(".*"\);?\s*$/m);
      const result = stripInsertCss(code, id)!;
      expect(result.library).toBe(library);
      expect(result.code).not.toMatch(/^__insertCSS\(/m);
    });
  }

  it("ignores other modules", () => {
    expect(stripInsertCss("__insertCSS(1)", "/x/node_modules/react/index.js")).toBeUndefined();
  });

  it("throws when a library no longer has the call, so an update that changes its shape fails the build", () => {
    expect(() => stripInsertCss("export {}", "/x/node_modules/sonner/dist/index.mjs")).toThrow(/no __insertCSS call/);
  });

  it("fails the bundle when a library was never transformed or a chunk still creates a <style>", () => {
    const plugin = staticLibraryCss();
    const generate = plugin.generateBundle as unknown as (this: unknown, options: unknown, bundle: Record<string, unknown>) => void;
    expect(() => generate.call({}, {}, {})).toThrow(/never transformed/);
    expect(RUNTIME_STYLE.test('const s=document.createElement("style");s.type="text/css"')).toBe(true);
    expect(RUNTIME_STYLE.test("document.createElement('div')")).toBe(false);
  });
});
