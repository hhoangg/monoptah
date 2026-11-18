import { readFileSync } from "node:fs";
import path from "node:path";
import { describe, expect, it } from "vitest";

const read = (...parts: string[]) => readFileSync(path.resolve(import.meta.dirname, ...parts), "utf8");
const tokens = read("../../styles/tokens.css");
const charts = read("charts.tsx");
const names = ["positive", "negative", "warning", "series-2", "series-3", "series-4", "series-5", "series-6"].map((name) => `--viz-${name}`);

describe("chart series hues are design tokens", () => {
  it("defines every hue for the light and the dark scheme", () => {
    const light = /:root,\s*:root\[data-scheme="light"\]\s*\{([^}]*--viz-positive[^}]*)\}/.exec(tokens)?.[1] ?? "";
    const dark = /:root\[data-scheme="dark"\]\s*\{([^}]*--viz-positive[^}]*)\}/.exec(tokens)?.[1] ?? "";
    for (const name of names) {
      expect(light, `light ${name}`).toMatch(new RegExp(`${name}:\\s*#[0-9a-f]{6}`));
      expect(dark, `dark ${name}`).toMatch(new RegExp(`${name}:\\s*#[0-9a-f]{6}`));
    }
  });

  it("charts read the tokens and keep no literal hue", () => {
    for (const name of names) expect(charts).toContain(`var(${name})`);
    expect(charts).not.toMatch(/#[0-9a-fA-F]{6}/);
  });
});
