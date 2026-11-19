import { describe, expect, it } from "vitest";
import { FIXTURE_HANDLERS, FixtureError, MockWorld, type HandlerContext, type MockDevice } from "../src/index.ts";
import { MENTION_MESSAGE, MOCK_PAGE_BYTES, PNG_BASE64, REPORT_HTML } from "../src/mentioned.ts";
import { setup } from "./harness.ts";

const device: MockDevice = { fingerprint: "ff", status: "confirmed", permission: "read", selection: { projects: null, monos: null } };
const ctx = (): HandlerContext => ({ world: new MockWorld(), device });
const ask = (params: { conversationId?: string; path: string; asset?: string; offset?: number }) =>
  FIXTURE_HANDLERS["chat.mentionedFile"]({ conversationId: "sess-local-files", ...params }, ctx());
const codeOf = (run: () => unknown) => {
  try {
    run();
  } catch (error) {
    return error instanceof FixtureError ? error.code : "other";
  }
  return "none";
};

describe("chat.mentionedFile fixtures", () => {
  it("seeds a session whose assistant message mentions every fixture", () => {
    const world = new MockWorld();
    const text = JSON.stringify(world.read("sess-local-files").turns);
    for (const mention of ["report.html", "docs/findings.md", "img/chart.png", "diagram.svg", "src/example.ts", "~/Documents/x/notes.py", "long-notes.txt", "bin/tool.bin", "big.log", "missing.txt", ".env"]) {
      expect(text).toContain(mention);
    }
    expect(MENTION_MESSAGE).toContain("`report.html`");
  });

  it("serves the sanitized html with asset placeholders and the asset list", () => {
    const result = ask({ path: "report.html" });
    expect(result).toMatchObject({ kind: "html", encoding: "utf8", name: "report.html", assets: ["img/chart.png"], project: { projectId: "proj-alpha", path: "report.html" } });
    expect(result.data).toBe(REPORT_HTML);
    expect(result.data).toContain("monoptah-asset:0");
    expect(result.data).not.toMatch(/<script|<style|onerror/i);
  });

  it("serves an asset of the html as a base64 image", () => {
    expect(ask({ path: "report.html", asset: "img/chart.png" })).toMatchObject({ kind: "image", mime: "image/png", encoding: "base64", data: PNG_BASE64 });
  });

  it("serves an svg as base64 bytes, a markdown file with its relative image, text and a binary without data", () => {
    expect(ask({ path: "diagram.svg" })).toMatchObject({ kind: "image", mime: "image/svg+xml", encoding: "base64" });
    expect(ask({ path: "docs/findings.md" })).toMatchObject({ kind: "markdown" });
    expect(ask({ path: "docs/findings.md", asset: "img/chart.png" })).toMatchObject({ kind: "image", mime: "image/png" });
    expect(ask({ path: "~/Documents/x/notes.py" })).toMatchObject({ kind: "text", path: "/Users/example/Documents/x/notes.py" });
    expect(ask({ path: "bin/tool.bin" })).toMatchObject({ kind: "binary", encoding: "none", data: "", size: 48_213 });
    expect(ask({ path: "./src/example.ts" })).toMatchObject({ kind: "text", name: "example.ts" });
  });

  it("pages long text and the pages concatenate to the file", () => {
    let offset = 0;
    let text = "";
    let pages = 0;
    for (;;) {
      const page = ask({ path: "long-notes.txt", offset });
      expect(page.offset).toBe(offset);
      expect(page.data.length).toBeLessThanOrEqual(MOCK_PAGE_BYTES);
      text += page.data;
      pages++;
      if (page.nextOffset === undefined) {
        expect(text.length).toBe(page.total);
        break;
      }
      offset = page.nextOffset;
    }
    expect(pages).toBeGreaterThan(2);
    expect(text.startsWith("001 Example line")).toBe(true);
  });

  it("answers the contract errors", () => {
    expect(codeOf(() => ask({ path: ".env" }))).toBe("path_denied");
    expect(codeOf(() => ask({ path: "big.log" }))).toBe("too_large");
    expect(codeOf(() => ask({ path: "missing.txt" }))).toBe("not_found");
    expect(codeOf(() => ask({ path: "etc/unmentioned.txt" }))).toBe("forbidden");
    expect(codeOf(() => ask({ path: "report.html", asset: "../secret.png" }))).toBe("forbidden");
    expect(codeOf(() => ask({ path: "report.html", asset: "/etc/passwd" }))).toBe("forbidden");
    expect(codeOf(() => ask({ path: "src/example.ts", asset: "x.png" }))).toBe("not_found");
    expect(codeOf(() => ask({ path: "docs/findings.md", asset: "../report.html" }))).toBe("forbidden");
    expect(() => ask({ conversationId: "nope", path: "report.html" })).toThrow(FixtureError);
  });

  it("is refused without the mentionedFiles capability and served with it, over the link", async () => {
    const without = await setup({ desktop: { capabilities: ["cloud", "approvals"] } });
    expect(await without.client.request("chat.mentionedFile", { conversationId: "sess-local-files", path: "report.html" })).toMatchObject({ ok: false });
    const withIt = await setup();
    const response = await withIt.client.request("chat.mentionedFile", { conversationId: "sess-local-files", path: "report.html" });
    expect(response).toMatchObject({ ok: true, result: { kind: "html" } });
  });
});
