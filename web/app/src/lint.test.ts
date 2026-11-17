import path from "node:path";
import { ESLint } from "eslint";
import { describe, expect, it } from "vitest";

// W-01 acceptance: ESLint fails on dangerouslySetInnerHTML, innerHTML and eval (security.md 8.1).
const cwd = path.resolve(import.meta.dirname, "..");
const eslint = new ESLint({ cwd, overrideConfigFile: path.join(cwd, "eslint.config.js") });

async function messages(code: string, file = "src/snippet.tsx") {
  const [result] = await eslint.lintText(code, { filePath: path.join(cwd, file) });
  return result?.messages ?? [];
}

describe("ESLint security bans", () => {
  it.each([
    ["dangerouslySetInnerHTML", "export const A = ({ h }: { h: string }) => <div dangerouslySetInnerHTML={{ __html: h }} />;"],
    ["innerHTML", "export function f(el: HTMLElement, h: string) { el.innerHTML = h; }"],
    ["outerHTML", "export function f(el: HTMLElement, h: string) { el.outerHTML = h; }"],
    ["insertAdjacentHTML", "export function f(el: HTMLElement, h: string) { el.insertAdjacentHTML('beforeend', h); }"],
    ["document.write", "export function f(h: string) { document.write(h); }"],
    ["eval", "export function f(s: string) { return eval(s); }"],
    ["window.eval", "export function f(s: string) { return window.eval(s); }"],
    ["new Function", "export function f(s: string) { return new Function(s); }"],
  ])("reports %s", async (_name, code) => {
    const found = await messages(code);
    expect(found.length, JSON.stringify(found)).toBeGreaterThan(0);
    expect(found.some((m) => m.severity === 2)).toBe(true);
  });

  it("cannot be silenced by an inline eslint-disable in src", async () => {
    for (const directive of [
      "/* eslint-disable no-restricted-properties */",
      "/* eslint-disable */",
      "// eslint-disable-next-line no-restricted-properties",
    ]) {
      const code = `${directive}\nexport function f(el: HTMLElement, h: string) { el.innerHTML = h; }\n`;
      const found = await messages(code);
      expect(found.some((m) => m.severity === 2 && m.message.includes("innerHTML")), directive).toBe(true);
    }
  });

  it("accepts ordinary React code", async () => {
    const found = await messages("export const A = ({ t }: { t: string }) => <p>{t}</p>;");
    expect(found).toEqual([]);
  });

  it("applies the react-hooks rules", async () => {
    const found = await messages(
      "import { useState } from 'react';\nexport function A({ c }: { c: boolean }) { if (c) { useState(0); } return null; }",
    );
    expect(found.map((m) => m.ruleId)).toContain("react-hooks/rules-of-hooks");
  });
});
