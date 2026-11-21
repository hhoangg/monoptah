// Terminal output is hostile input (security.md "Terminal"). These tests keep the terminal feature from growing a way for it
// to reach the DOM, the clipboard, the tab title, a link or storage other than through xterm and the user's own actions.
import { readFileSync, readdirSync } from "node:fs";
import path from "node:path";
import { ESLint } from "eslint";
import { describe, expect, it } from "vitest";

const dir = import.meta.dirname;
const appRoot = path.resolve(dir, "../../..");
const files = readdirSync(dir).filter((name) => /\.(ts|tsx)$/.test(name));
const sources = files.filter((name) => !/\.test\.tsx?$/.test(name) && name !== "testing.ts");
const read = (name: string): string => readFileSync(path.join(dir, name), "utf8");
/** Code with comments removed, so a rule explained in a comment does not trip the scan. */
const code = (name: string): string => read(name).replace(/\/\*[\s\S]*?\*\//g, "").replace(/(^|[^:])\/\/.*$/gm, "$1");

const BANNED: ReadonlyArray<[string, RegExp]> = [
  ["innerHTML", /\binnerHTML\b/],
  ["outerHTML", /\bouterHTML\b/],
  ["insertAdjacentHTML", /insertAdjacentHTML/],
  ["dangerouslySetInnerHTML", /dangerouslySetInnerHTML/],
  ["document.write", /document\.write/],
  ["setHTMLUnsafe and friends", /setHTMLUnsafe|parseHTMLUnsafe|createContextualFragment|DOMParser/],
  ["eval and Function", /\beval\s*\(|new Function\s*\(/],
  ["iframe", /<iframe|createElement\(["']iframe/],
  ["a script element", /<script|createElement\(["']script/],
];

describe("terminal feature sources", () => {
  it("has the files this test is meant to guard", () => {
    expect(sources).toEqual(expect.arrayContaining(["controller.ts", "xtermScreen.ts", "TerminalPane.tsx", "TerminalDrawer.tsx", "TerminalChooser.tsx", "session.ts", "sessions.ts", "KeyboardRow.tsx", "resume.ts"]));
  });

  it.each(sources)("%s uses no HTML, script or iframe sink", (name) => {
    for (const [label, pattern] of BANNED) expect(pattern.test(code(name)), `${name}: ${label}`).toBe(false);
  });

  // One linter for all files, and a generous timeout: the first file pays for loading the rules.
  const eslint = new ESLint({ cwd: appRoot, overrideConfigFile: path.join(appRoot, "eslint.config.js") });
  it.each(sources)(
    "%s passes the app's ESLint security bans",
    async (name) => {
      const [result] = await eslint.lintText(read(name), { filePath: path.join(dir, name) });
      expect(result?.messages.filter((message) => message.severity === 2).map((message) => `${message.ruleId}: ${message.message}`)).toEqual([]);
    },
    30_000,
  );

  it("has no one-time code UI or code step any more: the passkey ticket replaced it", () => {
    expect(files).not.toContain("CodeDialog.tsx");
    for (const name of sources) expect(code(name), name).not.toMatch(/challengeId|submitCode|CodeDialog|codeLength|terminal-code/);
  });

  it("never persists output: no local storage, IndexedDB or cache API in the terminal feature", () => {
    for (const name of sources) expect(/localStorage|indexedDB|\bcaches\b|CacheStorage|openDatabase/.test(code(name)), name).toBe(false);
  });

  it("keeps terminal state in sessionStorage as ids and counters only", () => {
    const text = code("resume.ts");
    expect(text).toContain("sessionStorage");
    const saved = /JSON\.stringify\(\{([^}]*)\}\)/.exec(text)?.[1] ?? "";
    expect(saved.split(",").map((part) => part.split(":")[0]?.trim())).toEqual(["terminalId", "next", "cwd", "idleTimeoutMs", "maxDurationMs", "openedAt"]);
  });

  it("loads xterm from npm by dynamic import, with the fit addon only: no links, clipboard or title handling", () => {
    const text = code("xtermScreen.ts");
    expect(text).toMatch(/import\("@xterm\/xterm"\)/);
    expect(text).toMatch(/import\("@xterm\/addon-fit"\)/);
    expect(text).not.toMatch(/addon-web-links|addon-clipboard|WebLinksAddon|ClipboardAddon|addon-image|addon-webgl/);
    expect(text).not.toMatch(/onTitleChange|document\.title/);
    expect(text).toMatch(/linkHandler:\s*\{\s*activate:\s*\(\)\s*=>\s*undefined\s*\}/);
    // No static import of xterm: it must stay out of the main chunk.
    expect(text).not.toMatch(/^import\s+[^t][^\n]*from\s+["']@xterm/m);
    for (const name of sources.filter((file) => file !== "xtermScreen.ts")) expect(code(name), name).not.toMatch(/from\s+["']@xterm/);
  });

  it("makes no network request of its own: terminal bytes travel only over the encrypted link", () => {
    for (const name of sources) expect(/\bfetch\s*\(|XMLHttpRequest|WebSocket|sendBeacon|EventSource/.test(code(name)), name).toBe(false);
  });

  it("uses no em or en dash and never names the upstream product", () => {
    for (const name of files) {
      expect(read(name), name).not.toMatch(new RegExp(`[${String.fromCharCode(0x2013, 0x2014)}]`));
      expect(read(name), name).not.toMatch(new RegExp(["Mono", "Code"].join("")));
    }
  });
});
