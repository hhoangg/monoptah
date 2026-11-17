// @vitest-environment node
import { mkdtempSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import path from "node:path";
import { describe, expect, it } from "vitest";
import { buildCsp, COMPANION_HEADERS } from "../../worker/src/csp.ts";
import { RP_ORIGIN_ENV, renderHeaders, resolveRpOrigin } from "./headers.ts";

const wrangler = path.resolve(import.meta.dirname, "../../wrangler.jsonc");

function tempJsonc(text: string): string {
  const file = path.join(mkdtempSync(path.join(tmpdir(), "monoptah-headers-")), "w.jsonc");
  writeFileSync(file, text);
  return file;
}

describe("_headers generation", () => {
  it("uses the RP_ORIGIN of wrangler.jsonc by default: connect-src 'self' wss://zptah.app", () => {
    const origin = resolveRpOrigin(wrangler, {});
    expect(origin).toBe("https://zptah.app");
    const text = renderHeaders(origin);
    expect(text).toContain("connect-src 'self' wss://zptah.app;");
    expect(text).toContain(`Content-Security-Policy: ${buildCsp("zptah.app")}`);
  });

  it("carries every companion header of the Worker and the cache rules", () => {
    const text = renderHeaders("https://zptah.app");
    for (const [name, value] of Object.entries(COMPANION_HEADERS)) expect(text).toContain(`  ${name}: ${value}`);
    expect(text).toContain("/assets/*\n  ! Cache-Control\n  Cache-Control: public, max-age=31536000, immutable");
  });

  it("honours the self-hoster override and keeps a port", () => {
    expect(resolveRpOrigin(wrangler, { [RP_ORIGIN_ENV]: "https://web.example.org" })).toBe("https://web.example.org");
    expect(renderHeaders("http://localhost:5173")).toContain("connect-src 'self' wss://localhost:5173;");
  });

  it("reads RP_ORIGIN from jsonc with comments, // inside strings and trailing commas", () => {
    const file = tempJsonc('{ // c\n "vars": { /* b */ "RP_ORIGIN": "https://a.example//x", }, }');
    expect(resolveRpOrigin(file, {})).toBe("https://a.example//x");
  });

  it("fails when RP_ORIGIN is missing", () => {
    expect(() => resolveRpOrigin(tempJsonc('{ "vars": {} }'), {})).toThrow(/RP_ORIGIN/);
  });
});
