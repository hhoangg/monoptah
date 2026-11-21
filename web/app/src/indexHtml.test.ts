import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { describe, expect, it } from "vitest";

const html = readFileSync(resolve(import.meta.dirname, "..", "index.html"), "utf8");
const meta = (name: string) => new RegExp(`<meta\\s+name="${name}"\\s+content="([^"]*)"`).exec(html)?.[1];

describe("index.html iOS home-screen meta", () => {
  // iOS 26: with "black-translucent" a home-screen app paints from y=0 under the status bar but its layout viewport
  // (100dvh) is 48pt (the status bar) shorter than the screen, so the bottom 48pt are never painted and the composer
  // is cut off. No CSS can draw into that band. "default" starts the web view below the status bar and runs it to the
  // screen bottom. Do not switch this back to black-translucent.
  it("uses the default status bar style", () => {
    expect(meta("apple-mobile-web-app-status-bar-style")).toBe("default");
  });
  // Without viewport-fit=cover the safe-area bottom reads 0 and the composer lands in the home indicator zone.
  it("keeps viewport-fit=cover", () => {
    expect(meta("viewport")).toContain("viewport-fit=cover");
  });
});

describe("index.html favicon links", () => {
  const hrefs = [...html.matchAll(/<link\s[^>]*rel="(?:icon|apple-touch-icon)"[^>]*>/g)].map((m) => /href="([^"]+)"/.exec(m[0])?.[1] as string);
  it("links the ico, svg and apple-touch icons", () => {
    expect(hrefs).toEqual(["/favicon.ico", "/favicon.svg", "/apple-touch-icon.png"]);
  });
  // A link to a file that is not in public/ would fall through to the SPA fallback and return the shell as the icon.
  const manifest = JSON.parse(readFileSync(resolve(import.meta.dirname, "..", "public", "manifest.webmanifest"), "utf8")) as { icons: { src: string }[] };
  it("points every link at a file in public/", () => {
    for (const href of [...hrefs, ...manifest.icons.map((i) => i.src)]) expect(() => readFileSync(resolve(import.meta.dirname, "..", "public", href.slice(1)))).not.toThrow();
  });
});
