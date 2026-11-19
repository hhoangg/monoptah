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
