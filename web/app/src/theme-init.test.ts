import { readFileSync } from "node:fs";
import path from "node:path";
import vm from "node:vm";
import { JSDOM } from "jsdom";
import { describe, expect, it } from "vitest";
import { THEME_BACKGROUNDS, THEMES } from "@/styles/themes";

const root = path.resolve(import.meta.dirname, "..");
const script = new vm.Script(readFileSync(path.join(root, "public/theme-init.js"), "utf8"));

/** Runs public/theme-init.js in a fresh document, as the browser does from <head>. */
function run(storage: Record<string, string>, systemDark: boolean) {
  const dom = new JSDOM('<!doctype html><html><head><meta name="theme-color" content="#131313"></head><body></body></html>', {
    runScripts: "outside-only",
    url: "https://monoptah.test/",
  });
  const win = dom.window as unknown as Window & typeof globalThis;
  for (const [k, v] of Object.entries(storage)) win.localStorage.setItem(k, v);
  win.matchMedia = ((q: string) => ({ matches: q.includes("dark") ? systemDark : false })) as typeof win.matchMedia;
  script.runInContext(dom.getInternalVMContext());
  const html = dom.window.document.documentElement;
  return {
    scheme: html.getAttribute("data-scheme"),
    theme: html.getAttribute("data-theme"),
    accent: html.getAttribute("data-accent"),
    color: dom.window.document.querySelector('meta[name="theme-color"]')?.getAttribute("content"),
  };
}

describe("public/theme-init.js", () => {
  it("defaults to system appearance, mono theme and theme accent", () => {
    expect(run({}, true)).toEqual({ scheme: "dark", theme: "mono", accent: "theme", color: "#131313" });
    expect(run({}, false)).toEqual({ scheme: "light", theme: "mono", accent: "theme", color: "#f7f7f7" });
  });

  it("reads the three monoptah.* keys", () => {
    expect(
      run({ "monoptah.appearance": "light", "monoptah.theme": "iris", "monoptah.accent": "orange" }, true),
    ).toEqual({ scheme: "light", theme: "iris", accent: "orange", color: "#f5f3f9" });
    expect(run({ "monoptah.appearance": "dark", "monoptah.theme": "grove" }, false)).toMatchObject({
      scheme: "dark",
      color: "#0e1612",
    });
  });

  it("ignores values outside the allowed lists", () => {
    expect(
      run({ "monoptah.appearance": "x", "monoptah.theme": "neon", "monoptah.accent": "<img>" }, false),
    ).toEqual({ scheme: "light", theme: "mono", accent: "theme", color: "#f7f7f7" });
  });

  it("keeps its background map in step with themes.ts and tokens.css", () => {
    const css = readFileSync(path.join(root, "src/styles/tokens.css"), "utf8");
    for (const theme of THEMES) {
      const [dark, light] = THEME_BACKGROUNDS[theme];
      for (const [scheme, expected] of [["dark", dark], ["light", light]] as const) {
        const selector =
          theme === "mono"
            ? new RegExp(`:root\\[data-scheme="${scheme}"\\] \\{[^}]*?--background: (#[0-9a-f]{6})`)
            : new RegExp(`:root\\[data-theme="${theme}"\\]\\[data-scheme="${scheme}"\\] \\{\\s*--background: (#[0-9a-f]{6})`);
        expect(css.match(selector)?.[1], `${theme} ${scheme}`).toBe(expected);
      }
      // The script resolves the same value for the same inputs.
      expect(run({ "monoptah.appearance": "dark", "monoptah.theme": theme }, true).color).toBe(dark);
      expect(run({ "monoptah.appearance": "light", "monoptah.theme": theme }, true).color).toBe(light);
    }
  });

  it("is a plain blocking script in index.html with no inline script or style", () => {
    const html = readFileSync(path.join(root, "index.html"), "utf8");
    expect(html).toMatch(/<head>[\s\S]*<script src="\/theme-init\.js"><\/script>[\s\S]*<\/head>/);
    expect(html).not.toMatch(/<script src="\/theme-init\.js"[^>]*(async|defer)/);
    expect(html).toContain('<meta name="color-scheme" content="light dark" />');
    const inlineScripts = [...html.matchAll(/<script(?![^>]*\bsrc=)[^>]*>/g)];
    expect(inlineScripts).toEqual([]);
    expect(html).not.toMatch(/<style|style=|onclick=/);
  });
});
