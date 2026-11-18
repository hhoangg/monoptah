import { readdirSync, readFileSync } from "node:fs";
import path from "node:path";
import { describe, expect, it } from "vitest";
import { ACCENTS, THEMES } from "@/styles/themes";
import { composite, computeVars, contrastRatio, parseColor, parseRules, relativeLuminance, resolveColor, type Selection } from "./contrast";

const tokensCss = readFileSync(path.resolve(import.meta.dirname, "../../styles/tokens.css"), "utf8");
const overridesCss = readFileSync(path.join(import.meta.dirname, "contrast-overrides.css"), "utf8");
const tokenRules = parseRules(tokensCss);
// The overrides load after tokens.css, so they win at equal specificity (register.ts imports them).
const rules = [...tokenRules, ...parseRules(overridesCss, tokenRules.length + 1000)];

const SCHEMES = ["light", "dark"] as const;
const TEXT = 4.5; // WCAG AA, normal text
const UI = 3; // WCAG AA, large text and UI components

const fmt = (n: number) => n.toFixed(2);
const table: string[] = [];

describe("contrast helpers", () => {
  it("matches the WCAG reference values", () => {
    expect(contrastRatio("#000000", "#ffffff")).toBeCloseTo(21, 5);
    expect(contrastRatio("#ffffff", "#ffffff")).toBeCloseTo(1, 5);
    expect(relativeLuminance(parseColor("#ffffff"))).toBeCloseTo(1, 5);
    // design-system.md section 9: the old light muted #737373 on #f7f7f7 is 4.4.
    expect(contrastRatio("#737373", "#f7f7f7")).toBeCloseTo(4.4, 1);
  });

  it("composites translucent colours over the background", () => {
    const over = composite(parseColor("rgba(0, 0, 0, 0.5)"), parseColor("#ffffff"));
    expect(Math.round(over.r)).toBe(128);
  });
});

describe("theme contrast (measured from tokens.css and contrast-overrides.css)", () => {
  for (const scheme of SCHEMES) {
    for (const theme of THEMES) {
      it(`${theme}/${scheme}: content, muted and ladder text on the grounds`, () => {
        const v = computeVars(rules, { scheme, theme, accent: "theme" });
        const color = (name: string) => resolveColor(name, v);
        // The ladder step essential translucent text lands on after the section 9 override.
        const ladder = scheme === "light" ? "--content-a72" : "--content-a50";
        for (const ground of ["--background", "--surface", "--sheet", "--list-card"]) {
          const bg = color(ground);
          const content = contrastRatio(color("--content"), bg);
          const muted = contrastRatio(color("--muted"), bg);
          const alpha = contrastRatio(color(ladder), bg);
          table.push(`${theme} | ${scheme} | ${ground} | content ${fmt(content)} / muted ${fmt(muted)} / ${ladder} ${fmt(alpha)}`);
          expect(content, `content on ${ground}`).toBeGreaterThanOrEqual(TEXT);
          expect(muted, `muted on ${ground}`).toBeGreaterThanOrEqual(TEXT);
          expect(alpha, `${ladder} on ${ground}`).toBeGreaterThanOrEqual(TEXT);
        }
        for (const name of ["--additions", "--deletions", "--caution", "--file-ref"]) {
          const ratio = contrastRatio(color(name), color("--background"));
          table.push(`${theme} | ${scheme} | ${name} on --background | ${fmt(ratio)}`);
          expect(ratio, name).toBeGreaterThanOrEqual(TEXT);
        }
      });
    }
  }
});

describe("accent contrast (measured from tokens.css and contrast-overrides.css)", () => {
  for (const scheme of SCHEMES) {
    for (const theme of THEMES) {
      for (const accent of ACCENTS) {
        it(`${theme}/${scheme}/${accent}: accent-text >= 4.5 and accent-color >= 3`, () => {
          const sel: Selection = { scheme, theme, accent };
          const v = computeVars(rules, sel);
          for (const ground of ["--background", "--surface"]) {
            const bg = resolveColor(ground, v);
            const text = contrastRatio(resolveColor("--accent-text", v), bg);
            const ui = contrastRatio(resolveColor("--accent-color", v), bg);
            table.push(`${theme} | ${scheme} | accent ${accent} on ${ground} | text ${fmt(text)} / ui ${fmt(ui)}`);
            expect(text, `accent-text on ${ground}`).toBeGreaterThanOrEqual(TEXT);
            expect(ui, `accent-color on ${ground}`).toBeGreaterThanOrEqual(UI);
          }
        });
      }
    }
  }

describe("essential text uses the muted token, not the decorative faint one", () => {
  for (const scheme of SCHEMES) {
    for (const theme of THEMES) {
      it(`${theme}/${scheme}: faint is measured as decorative, muted passes on every ground`, () => {
        const v = computeVars(rules, { scheme, theme, accent: "theme" });
        for (const ground of ["--background", "--surface", "--sheet", "--list-card"]) {
          const bg = resolveColor(ground, v);
          const faint = contrastRatio(resolveColor("--faint", v), bg);
          const muted = contrastRatio(resolveColor("--muted", v), bg);
          table.push(`${theme} | ${scheme} | ${ground} | faint ${fmt(faint)} (decorative only) / muted ${fmt(muted)}`);
          // Faint stays below the text bar: the markup test below is what keeps text off it.
          expect(faint, `faint on ${ground} is decorative`).toBeLessThan(TEXT);
          expect(muted, `muted on ${ground}`).toBeGreaterThanOrEqual(TEXT);
        }
      });
    }
  }
});

describe("text-faint is for decorative elements only", () => {
  const srcRoot = path.resolve(import.meta.dirname, "../..");
  const sources = (dir: string): string[] =>
    readdirSync(dir, { withFileTypes: true }).flatMap((entry) => {
      const full = path.join(dir, entry.name);
      if (entry.isDirectory()) return entry.name === "node_modules" ? [] : sources(full);
      return /\.tsx$/.test(entry.name) && !/\.test\.tsx$/.test(entry.name) ? [full] : [];
    });
  // A faint hit is allowed on an icon or an aria-hidden element (a gutter number), never on text a person reads.
  const decorative = (line: string) => /<Icon\b/.test(line) || /aria-hidden/.test(line);

  it("finds no essential text with text-faint or placeholder:text-faint", () => {
    const offenders = sources(srcRoot).flatMap((file) =>
      readFileSync(file, "utf8")
        .split("\n")
        .flatMap((line, index) => (/(?<![-\w])(placeholder:)?text-faint\b/.test(line) && !decorative(line) ? [`${path.relative(srcRoot, file)}:${index + 1}: ${line.trim()}`] : [])),
    );
    expect(offenders).toEqual([]);
  });
});

  it("lists the measured ratio of every combination", () => {
    console.info(`theme | scheme | pair | ratio\n${table.join("\n")}`);
    expect(table.length).toBeGreaterThan(0);
  });
});
