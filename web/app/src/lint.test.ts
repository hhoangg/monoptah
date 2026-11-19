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
    ["setHTMLUnsafe", "export function f(el: HTMLElement, h: string) { (el as unknown as { setHTMLUnsafe(h: string): void }).setHTMLUnsafe(h); }"],
    ["Document.parseHTMLUnsafe", "export function f(h: string) { return (Document as unknown as { parseHTMLUnsafe(h: string): Document }).parseHTMLUnsafe(h); }"],
    ["createContextualFragment", "export function f(h: string) { return document.createRange().createContextualFragment(h); }"],
    ["new DOMParser", "export function f() { return new DOMParser(); }"],
    ["parseFromString", "export function f(p: DOMParser, h: string) { return p.parseFromString(h, 'text/html'); }"],
    ["iframe srcdoc property", "export function f(el: HTMLIFrameElement, h: string) { el.srcdoc = h; }"],
    ["JSX srcDoc attribute", "export const A = ({ h }: { h: string }) => <iframe srcDoc={h} />;"],
    ["setAttribute style", "export function f(el: HTMLElement, v: string) { el.setAttribute('style', v); }"],
    ["setAttribute srcdoc", "export function f(el: HTMLElement, v: string) { el.setAttribute('srcdoc', v); }"],
    ["setAttribute onclick", "export function f(el: HTMLElement, v: string) { el.setAttribute('onclick', v); }"],
    ["new Worker outside the allowlist", "export function f() { return new Worker('x.js'); }"],
    ["new SharedWorker", "export function f() { return new SharedWorker('x.js'); }"],
    ["setAttributeNS onload", "export function f(el: Element, v: string) { el.setAttributeNS(null, 'onload', v); }"],
    ["createElementNS script", "export function f(ns: string) { return document.createElementNS(ns, 'script'); }"],
    ["new globalThis.Function", "export function f(s: string) { return new globalThis.Function(s); }"],
    ["new window.DOMParser", "export function f() { return new window.DOMParser(); }"],
    ["new window.Worker", "export function f() { return new window.Worker('x.js'); }"],
    ["importScripts", "export function f() { importScripts('x.js'); }"],
    ["createElement script", "export function f() { return document.createElement('script'); }"],
    ["JSX script element", "export const A = () => <script />;"],
    ["script src", "export function f(script: HTMLScriptElement, u: string) { script.src = u; }"],
    ["script textContent", "export function f(script: HTMLScriptElement, u: string) { script.textContent = u; }"],
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

  it("allows new Worker only in the allowlisted modules, and still bans the other sinks there", async () => {
    const worker = "export function f() { return new Worker(new URL('./x.worker.ts', import.meta.url), { type: 'module' }); }";
    expect(await messages(worker, "src/features/backgrounds/render.ts")).toEqual([]);
    expect(await messages(worker, "src/features/markdown/highlight/plugin.ts")).toEqual([]);
    expect((await messages(worker, "src/features/other/thing.ts")).length).toBeGreaterThan(0);
    const sink = await messages("export function f(h: string) { return new DOMParser().parseFromString(h, 'text/html'); }", "src/features/backgrounds/render.ts");
    expect(sink.some((m) => m.severity === 2)).toBe(true);
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
