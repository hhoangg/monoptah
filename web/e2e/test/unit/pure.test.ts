import { readFileSync } from "node:fs";
import { join } from "node:path";
import { describe, expect, it } from "vitest";
import {
  base64urlDecode,
  base64urlEncode,
  clearRateLimitsArgs,
  findCredentialLeaks,
  decideDevicesProbe,
  decideRouteProbe,
  decideHeadlessProbe,
  decidePairProbe,
  decidePairingProbe,
  decideUiProbe,
  parseCapabilities,
  parseInviteCode,
  resolveHeadlessBin,
  testIp,
} from "../../lib/pure.ts";
import { outputDir, webDir } from "../../lib/paths.ts";

describe("parseInviteCode", () => {
  it("reads the code from the printed link", () => {
    const out = "Bootstrap invite (one use, 24 h):\nhttp://localhost:8787/register#invite=AbC_-123xyz\nDo not share.\n";
    expect(parseInviteCode(out)).toBe("AbC_-123xyz");
  });
  it("returns null when there is no link", () => {
    expect(parseInviteCode("nothing here")).toBeNull();
    expect(parseInviteCode("#invite=")).toBeNull();
  });
});

describe("parseCapabilities", () => {
  it("parses the capabilities line, ignoring log lines", () => {
    const out = 'starting\n{"version":1,"serve":false,"reason":"not implemented","checkVectors":true}\n';
    expect(parseCapabilities(out)).toEqual({ version: 1, serve: false, pair: false, reason: "not implemented", checkVectors: true });
    expect(parseCapabilities('{"version":1,"serve":false,"pair":true,"reason":null,"checkVectors":true}')?.pair).toBe(true);
  });
  it("accepts a null reason", () => {
    expect(parseCapabilities('{"version":1,"serve":true,"reason":null,"checkVectors":true}')?.serve).toBe(true);
  });
  it("rejects wrong shapes and garbage", () => {
    expect(parseCapabilities("")).toBeNull();
    expect(parseCapabilities("{not json}")).toBeNull();
    expect(parseCapabilities('{"version":1,"serve":"yes","reason":null,"checkVectors":true}')).toBeNull();
    expect(parseCapabilities('{"version":1,"serve":true,"reason":5,"checkVectors":true}')).toBeNull();
    expect(parseCapabilities("[1,2]")).toBeNull();
  });
});

describe("base64url helpers", () => {
  it("round-trips bytes without padding", () => {
    const bytes = new Uint8Array([251, 255, 254, 0, 1, 2, 3]);
    const text = base64urlEncode(bytes);
    expect(text).not.toMatch(/[+/=]/);
    expect(Array.from(base64urlDecode(text))).toEqual(Array.from(bytes));
  });
  it("rejects characters outside the alphabet", () => {
    expect(() => base64urlDecode("a+b/")).toThrow();
  });
});

describe("testIp", () => {
  it("builds TEST-NET-2 addresses and rejects out of range", () => {
    expect(testIp(7)).toBe("198.51.100.7");
    expect(() => testIp(0)).toThrow();
    expect(() => testIp(255)).toThrow();
  });
});

describe("resolveHeadlessBin", () => {
  it("defaults to the native-dev build", () => {
    expect(resolveHeadlessBin("/repo", {})).toBe("/repo/crates/monocode-gpui/target/native-dev/web_link_headless");
  });
  it("honours E2E_HEADLESS_BIN, relative to the repo or absolute", () => {
    expect(resolveHeadlessBin("/repo", { E2E_HEADLESS_BIN: "bin/x" })).toBe("/repo/bin/x");
    expect(resolveHeadlessBin("/repo", { E2E_HEADLESS_BIN: "/opt/x" })).toBe("/opt/x");
  });
});

describe("outputDir", () => {
  it("defaults outside the repo and honours E2E_OUTPUT_DIR", () => {
    expect(outputDir({})).toBe("/tmp/evidence/ws-q/e2e");
    expect(outputDir({ E2E_OUTPUT_DIR: "/tmp/x" })).toBe("/tmp/x");
  });
});

describe("probe decisions", () => {
  const unsupported = { status: 501, code: "unsupported" };
  it("pairing: only 501 unsupported and 404 skip; validation and success run", () => {
    for (const answer of [unsupported, { status: 404, code: null }]) {
      const probe = decidePairingProbe(answer);
      expect(probe.run).toBe(false);
      expect(probe.reason).toContain("TODO(E-01:WS-R R-03)");
    }
    expect(decidePairingProbe({ status: 400, code: "invalid" }).run).toBe(true);
    expect(decidePairingProbe({ status: 200, code: null }).run).toBe(true);
  });
  it("devices: same rule with the R-04 marker; 403 bad_origin is never 'present'", () => {
    expect(decideDevicesProbe(unsupported)).toMatchObject({ run: false });
    expect(decideDevicesProbe(unsupported).reason).toContain("TODO(E-01:WS-R R-04)");
    expect(() => decideDevicesProbe({ status: 403, code: "bad_origin" })).toThrow(/unexpected answer/);
    expect(decideDevicesProbe({ status: 400, code: "invalid" }).run).toBe(true);
  });
  it("a 501 without the unsupported code, a 5xx, a 429 and no answer all fail the probe", () => {
    expect(() => decideRouteProbe("x", "TODO", "POST /x", { status: 501, code: "other" })).toThrow();
    expect(() => decideRouteProbe("x", "TODO", "POST /x", { status: 500, code: null })).toThrow();
    expect(() => decideRouteProbe("x", "TODO", "POST /x", { status: 429, code: "rate_limited" })).toThrow();
    expect(() => decidePairingProbe(null)).toThrow(/no answer/);
    expect(() => decideDevicesProbe(null)).toThrow(/no answer/);
  });
  it("headless: needs the binary and serve=true", () => {
    const caps = { version: 1, serve: true, pair: true, reason: null, checkVectors: true };
    expect(decideHeadlessProbe(false, null).run).toBe(false);
    expect(decideHeadlessProbe(true, null).run).toBe(false);
    expect(decideHeadlessProbe(true, { ...caps, serve: false, reason: "later" }).reason).toContain("later");
    expect(decideHeadlessProbe(true, caps).run).toBe(true);
    expect(decideHeadlessProbe(false, null).reason).toContain("TODO(E-01:WS-D1 R-05)");
    expect(decideHeadlessProbe(true, { ...caps, serve: false }).reason).toContain("R-05, R-15");
  });
  it("pair: needs the binary and pair=true, independent of serve", () => {
    const caps = { version: 1, serve: false, pair: true, reason: "later", checkVectors: true };
    expect(decidePairProbe(false, null).run).toBe(false);
    expect(decidePairProbe(true, null).run).toBe(false);
    expect(decidePairProbe(true, { ...caps, pair: false }).run).toBe(false);
    expect(decidePairProbe(true, caps).run).toBe(true);
  });
  const page = { isPlaceholder: false, hasRoot: true, hasRouteStub: false, rootHasContent: true, pageErrors: [] as string[] };
  it("ui: RouteStub and the placeholder skip, a rendered app runs", () => {
    expect(decideUiProbe({ ...page, hasRouteStub: true }).run).toBe(false);
    expect(decideUiProbe({ ...page, hasRouteStub: true }).reason).toContain("TODO(E-01:WS-S W-02)");
    expect(decideUiProbe({ ...page, isPlaceholder: true, hasRoot: false, rootHasContent: false }).run).toBe(false);
    expect(decideUiProbe(page).run).toBe(true);
  });
  it("ui: a served app that renders nothing fails with the page errors", () => {
    expect(() => decideUiProbe({ ...page, rootHasContent: false, pageErrors: ["boom"] })).toThrow(/boom/);
    expect(() => decideUiProbe({ ...page, hasRoot: false, rootHasContent: false })).toThrow(/neither RouteStub/);
  });
});

describe("clearRateLimitsArgs", () => {
  const args = clearRateLimitsArgs("state-dir", "wrangler.jsonc");
  it("runs one statement on the local database of the harness persistence dir, and nothing else", () => {
    expect(args.slice(0, 3)).toEqual(["d1", "execute", "monoptah"]);
    expect(args).toContain("--local");
    expect(args[args.indexOf("--persist-to") + 1]).toBe("state-dir");
    expect(args[args.indexOf("-c") + 1]).toBe("wrangler.jsonc");
    expect(args[args.indexOf("--command") + 1]).toBe("DELETE FROM rate_limits");
    expect(args).not.toContain("--remote");
  });
  it("names a table that the Worker migrations create", () => {
    const sql = readFileSync(join(webDir, "worker", "migrations", "0001_init.sql"), "utf8");
    expect(sql).toContain("CREATE TABLE `rate_limits`");
  });
});

describe("findCredentialLeaks", () => {
  const fingerprint = "a".repeat(64);
  const id = "6f1c2d3e-aaaa-bbbb-cccc-123456789abc";
  it("accepts the events of pair and serve", () => {
    const out = [
      JSON.stringify({ event: "pairing", userCode: "ABCD-EFGH", confirmationCode: "123456", fingerprint, link: `http://localhost/pair/ABCD-EFGH#fp=${fingerprint}`, expiresIn: 600 }),
      JSON.stringify({ event: "paired", desktopId: id, account: "x", fingerprint }),
      JSON.stringify({ event: "online", desktopId: id, fingerprint }),
      JSON.stringify({ event: "request", method: "workspace.list" }),
    ].join("\n");
    expect(findCredentialLeaks(out)).toEqual([]);
    expect(findCredentialLeaks("")).toEqual([]);
  });
  it("finds a token-like run, in a JSON field or in free text, without echoing it", () => {
    const token = "Zk3-".repeat(11);
    for (const text of [JSON.stringify({ event: "error", message: `bad ${token}` }), `panic: ${token}`, "b".repeat(40)]) {
      const found = findCredentialLeaks(text);
      expect(found.length).toBeGreaterThan(0);
      expect(found.join(" ")).not.toContain(token);
    }
  });
  it("finds the credential field names", () => {
    for (const text of ['{"desktopToken":"x"}', "deviceCode=abc", "verifier: abc", "Authorization: Bearer abc"]) {
      expect(findCredentialLeaks(text).length).toBeGreaterThan(0);
    }
  });
});
