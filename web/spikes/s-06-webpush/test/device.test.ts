// The device test page must work under the production CSP (web/worker/src/csp.ts, imported read-only): only same-origin
// script files, no inline script, no inline event handlers, no HTML sinks, no third-party origins.
import { readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { describe, expect, it } from "vitest";
import { buildCsp } from "../../../worker/src/csp.ts";

const dir = join(dirname(fileURLToPath(import.meta.url)), "..", "device");
const read = (name: string): string => readFileSync(join(dir, name), "utf8");

describe("device test page vs the production CSP", () => {
  const html = read("index.html");
  const csp = buildCsp("zptah.app");

  it("uses only external same-origin scripts and styles", () => {
    expect(csp).toContain("script-src 'self'");
    for (const tag of html.match(/<script\b[^>]*>/g) ?? []) expect(tag).toMatch(/\bsrc="[^":]+"/);
    expect(html).not.toMatch(/<script\b[^>]*>\s*[^<\s]/); // no inline body
    expect(html).not.toMatch(/<style\b/);
    expect(html).not.toMatch(/\son[a-z]+\s*=/i); // no inline event handlers
    expect(html).not.toMatch(/\sstyle\s*=/i);
    expect(html).not.toMatch(/(?:src|href)="(?:https?:)?\/\//);
  });

  it("avoids HTML and code-string sinks in the scripts", () => {
    for (const file of ["app.js", "sw.js"]) {
      const source = read(file).replace(/\/\*[\s\S]*?\*\//g, "").replace(/^\s*\/\/.*$/gm, ""); // comments may name the banned APIs
      expect(source).not.toMatch(/innerHTML|outerHTML|insertAdjacentHTML|document\.write|\beval\(|new Function/);
    }
  });

  it("only talks to its own origin from the page (connect-src 'self')", () => {
    expect(read("app.js")).not.toMatch(/fetch\(|XMLHttpRequest|WebSocket/);
  });
});
