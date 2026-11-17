import { readdirSync, readFileSync, statSync } from "node:fs";
import path from "node:path";
import { describe, expect, it } from "vitest";
import { renderHeaders, resolveRpOrigin } from "../../build/headers";

const src = path.resolve(import.meta.dirname, "..");

function walk(dir: string, filter: (file: string) => boolean): string[] {
  return readdirSync(dir).flatMap((name) => {
    const full = path.join(dir, name);
    if (statSync(full).isDirectory()) return walk(full, filter);
    return filter(full) ? [full] : [];
  });
}

describe("design tokens", () => {
  const cssFiles = walk(path.join(src, "styles"), (f) => f.endsWith(".css"));

  it("has no box-shadow other than none (design-system.md ground rule)", () => {
    expect(cssFiles.length).toBeGreaterThan(0);
    for (const file of cssFiles) {
      const css = readFileSync(file, "utf8").replace(/\/\*[\s\S]*?\*\//g, "");
      for (const match of css.matchAll(/box-shadow\s*:\s*([^;}]*)/g)) {
        const value = (match[1] ?? "").replace("!important", "").trim();
        expect(value, `${path.basename(file)}: box-shadow`).toBe("none");
      }
      expect(css, `${path.basename(file)}: drop-shadow filter`).not.toMatch(/drop-shadow\(/);
    }
  });

  it("removes every shadow utility through the theme", () => {
    const css = readFileSync(path.join(src, "styles/tailwind.css"), "utf8");
    expect(css).toContain("--shadow-*: initial;");
    expect(css).toContain("--drop-shadow-*: initial;");
  });

  it("has no shadow-* utility classes in components or routes", () => {
    const files = walk(src, (f) => /\.(tsx|ts)$/.test(f) && !f.endsWith(".test.ts") && !f.endsWith(".test.tsx"));
    for (const file of files) {
      const code = readFileSync(file, "utf8").replace(/\/\*[\s\S]*?\*\//g, "").replace(/^\s*\/\/.*$/gm, "");
      const found = code.match(/["'`\s](?:[a-z-]+:)*(?:drop-)?shadow(?:-(?!none["'`\s])[^\s"'`]*)?(?=["'`\s])/g) ?? [];
      expect(found, path.relative(src, file)).toEqual([]);
    }
  });

  it("defines the palette for every theme, accent and both schemes", () => {
    const css = readFileSync(path.join(src, "styles/tokens.css"), "utf8");
    for (const theme of ["ocean", "grove", "ember", "iris", "rose"]) {
      for (const scheme of ["light", "dark"]) {
        expect(css).toContain(`:root[data-theme="${theme}"][data-scheme="${scheme}"]`);
      }
    }
    for (const accent of ["blue", "teal", "green", "orange", "red", "pink", "violet"]) {
      for (const scheme of ["light", "dark"]) {
        expect(css).toContain(`:root[data-accent="${accent}"][data-scheme="${scheme}"]`);
      }
    }
    expect(css).toContain("--accent-text: color-mix(in srgb, var(--accent-color) 85%, black);");
    expect(css).toContain("--muted: #666666;");
  });

  it("keeps the Tailwind @theme block of design-system.md 5.3 (brand names, radii, scale)", () => {
    const css = readFileSync(path.join(src, "styles/tailwind.css"), "utf8");
    for (const needle of [
      "@theme inline {",
      "--color-muted-text: var(--muted);",
      "--color-brand: var(--accent-color);",
      "--radius-pill: 999px;",
      "--radius-sheet: 32px;",
      "--text-row: 1.0625rem;",
      "--breakpoint-lg: 64rem;",
    ]) {
      expect(css).toContain(needle);
    }
  });
});

describe("static asset headers (generated at build time, see build/headers.ts)", () => {
  const headers = renderHeaders(resolveRpOrigin(path.resolve(src, "../../wrangler.jsonc"), {}));
  const csp = headers.match(/^\s+Content-Security-Policy: (.*)$/m)?.[1] ?? "";

  it("carries the CSP of security.md 8.1 with connect-src 'self' wss://HOST", () => {
    for (const directive of [
      "default-src 'none'",
      "script-src 'self'",
      "style-src 'self'",
      "style-src-attr 'unsafe-inline'",
      "connect-src 'self' wss://zptah.app;",
      "worker-src 'self'",
      "frame-ancestors 'none'",
      "require-trusted-types-for 'script'",
      "trusted-types default",
    ]) {
      expect(csp).toContain(directive);
    }
    expect(csp).not.toMatch(/script-src[^;]*unsafe-inline/);
  });

  it("caches hashed assets for a year and revalidates the rest", () => {
    expect(headers).toMatch(/\/\*\n(?: {2}.*\n)*? {2}Cache-Control: no-cache/);
    expect(headers).toMatch(/\/assets\/\*\n {2}! Cache-Control\n {2}Cache-Control: public, max-age=31536000, immutable/);
  });
});
