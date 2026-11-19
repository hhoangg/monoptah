import { readFileSync } from "node:fs";
import path from "node:path";
import { describe, expect, it } from "vitest";
import { ACCENTS, THEMES } from "@/styles/themes";
import { computeVars, parseRules } from "./contrast";
import { accentSwatch, themeSwatch } from "./swatches";

const rules = parseRules(readFileSync(path.resolve(import.meta.dirname, "../../styles/tokens.css"), "utf8"));

describe("picker swatches", () => {
  for (const scheme of ["light", "dark"] as const) {
    it(`match tokens.css in ${scheme}`, () => {
      for (const theme of THEMES) {
        const vars = computeVars(rules, { scheme, theme, accent: "theme" });
        expect(themeSwatch(theme, scheme)).toEqual({ background: vars["--background"], accent: vars["--accent-color"] });
      }
      for (const accent of ACCENTS) {
        const vars = computeVars(rules, { scheme, theme: "ocean", accent });
        expect(accentSwatch(accent, "ocean", scheme)).toBe(vars["--accent-color"]);
      }
    });
  }
});
