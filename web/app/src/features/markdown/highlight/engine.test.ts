import { describe, expect, it } from "vitest";
import { createHighlightEngine } from "./engine";
import { normalizeLanguage, supportedLanguages } from "./languages";
import { MAX_HIGHLIGHT_CHARS } from "./types";

describe("highlight engine", () => {
  it("tokenizes typescript into coloured runs for both themes", async () => {
    const engine = createHighlightEngine();
    const result = await engine.highlight({ code: "const a: number = 1;\nreturn a;", language: "ts" });
    expect(result?.tokens).toHaveLength(2);
    const first = result?.tokens[0] ?? [];
    expect(first.map((token) => token.content).join("")).toBe("const a: number = 1;");
    expect(first.some((token) => token.light && token.dark && token.light !== token.dark)).toBe(true);
    for (const token of first) {
      if (token.light) expect(token.light).toMatch(/^#[0-9a-f]{3,8}$/i);
    }
  });

  it("returns null for unknown languages and oversized blocks", async () => {
    const engine = createHighlightEngine();
    expect(await engine.highlight({ code: "x", language: "brainfuck" })).toBeNull();
    expect(await engine.highlight({ code: "x", language: "" })).toBeNull();
    expect(await engine.highlight({ code: "a".repeat(MAX_HIGHLIGHT_CHARS + 1), language: "js" })).toBeNull();
  });

  it("resumes from the checkpoint while a fence grows, with the same tokens as a fresh run", async () => {
    const streaming = createHighlightEngine();
    const full = "function f() {\n  return `a ${1}`;\n}\nconst x = 2;";
    await streaming.highlight({ code: "function f() {\n  ret", language: "typescript" });
    await streaming.highlight({ code: "function f() {\n  return `a ${1}`;\n}\nco", language: "typescript" });
    const grown = await streaming.highlight({ code: full, language: "typescript" });
    const fresh = await createHighlightEngine({ maxEntries: 0 }).highlight({ code: full, language: "typescript" });
    expect(grown).toEqual(fresh);
  });

  it("bounds its cache", async () => {
    const engine = createHighlightEngine({ maxEntries: 2 });
    for (const code of ["a", "b", "c", "d"]) await engine.highlight({ code, language: "js" });
    expect(engine.cachedResults()).toBeLessThanOrEqual(2);
  });

  it("keeps a malicious fence tag out of the loader", () => {
    expect(normalizeLanguage("__proto__")).toBeUndefined();
    expect(normalizeLanguage("constructor")).toBeUndefined();
    expect(normalizeLanguage("../../evil")).toBeUndefined();
    expect(normalizeLanguage("TS")).toBe("typescript");
    expect(supportedLanguages().length).toBeGreaterThan(20);
  });
});
