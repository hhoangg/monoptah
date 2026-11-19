import { ESLint } from "eslint";
import { describe, expect, it } from "vitest";

// The shared base (web/eslint.config.js) bans the HTML and code-string sinks of
// security.md section 8 item 1. This proves each ban fires, so a refactor of the
// config cannot silently drop one.
const eslint = new ESLint({ cwd: new URL("../", import.meta.url).pathname });

async function messages(code: string, filePath = "src/probe.tsx"): Promise<string[]> {
  const [result] = await eslint.lintText(code, { filePath });
  return (result?.messages ?? []).map((message) => `${message.ruleId}: ${message.message}`);
}

describe("shared eslint bans (security.md 8 item 1)", () => {
  const banned: [string, string][] = [
    ["dangerouslySetInnerHTML", "export const a = <div dangerouslySetInnerHTML={{ __html: x }} />;"],
    ["innerHTML assignment", "declare const el: HTMLElement; el.innerHTML = 'x';"],
    ["innerHTML read", "declare const el: HTMLElement; export const s = el.innerHTML;"],
    ["innerHTML by string key", "declare const el: HTMLElement; el['innerHTML'] = 'x';"],
    ["outerHTML", "declare const el: HTMLElement; el.outerHTML = 'x';"],
    ["insertAdjacentHTML", "declare const el: HTMLElement; el.insertAdjacentHTML('beforeend', 'x');"],
    ["document.write", "declare const document: { write(s: string): void }; document.write('x');"],
    ["eval", "export const v = eval('1');"],
    ["window.eval", "declare const window: { eval(s: string): unknown }; window.eval('1');"],
    ["new Function", "export const f = new Function('return 1');"],
    ["Function()", "export const f = Function('return 1');"],
  ];
  for (const [name, code] of banned) {
    it(`rejects ${name}`, async () => {
      const found = await messages(code);
      expect(found.length, found.join("\n")).toBeGreaterThan(0);
    });
  }

  it("accepts ordinary code", async () => {
    expect(await messages("export const a = [1, 2, 3].map((n) => n * 2);\n", "src/ok.ts")).toEqual([]);
  });
});
