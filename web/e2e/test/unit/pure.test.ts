import { describe, expect, it } from "vitest";
import {
  base64urlDecode,
  base64urlEncode,
  decideDevicesProbe,
  decideHeadlessProbe,
  decidePairingProbe,
  decideUiProbe,
  parseCapabilities,
  parseInviteCode,
  resolveHeadlessBin,
  testIp,
} from "../../lib/pure.ts";
import { outputDir } from "../../lib/paths.ts";

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
    expect(parseCapabilities(out)).toEqual({ version: 1, serve: false, reason: "not implemented", checkVectors: true });
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
  it("pairing: 501, 404 and no answer skip; anything else runs", () => {
    for (const status of [501, 404, null]) {
      const probe = decidePairingProbe(status);
      expect(probe.run).toBe(false);
      expect(probe.reason).toContain("TODO(E-01:WS-R R-03)");
    }
    expect(decidePairingProbe(400).run).toBe(true);
    expect(decidePairingProbe(200).run).toBe(true);
  });
  it("devices: same rule with the R-04 marker", () => {
    expect(decideDevicesProbe(501)).toMatchObject({ run: false });
    expect(decideDevicesProbe(501).reason).toContain("TODO(E-01:WS-R R-04)");
    expect(decideDevicesProbe(401).run).toBe(true);
  });
  it("headless: needs the binary and serve=true", () => {
    const caps = { version: 1, serve: true, reason: null, checkVectors: true };
    expect(decideHeadlessProbe(false, null).run).toBe(false);
    expect(decideHeadlessProbe(true, null).run).toBe(false);
    expect(decideHeadlessProbe(true, { ...caps, serve: false, reason: "later" }).reason).toContain("later");
    expect(decideHeadlessProbe(true, caps).run).toBe(true);
    expect(decideHeadlessProbe(false, null).reason).toContain("TODO(E-01:WS-D1 R-05)");
  });
  it("ui: stub or placeholder skips, a rendered app runs", () => {
    expect(decideUiProbe({ hasRoot: true, hasRouteStub: true, rootHasContent: true }).run).toBe(false);
    expect(decideUiProbe({ hasRoot: false, hasRouteStub: false, rootHasContent: false }).run).toBe(false);
    expect(decideUiProbe({ hasRoot: true, hasRouteStub: false, rootHasContent: false }).run).toBe(false);
    expect(decideUiProbe({ hasRoot: true, hasRouteStub: false, rootHasContent: true }).run).toBe(true);
    expect(decideUiProbe({ hasRoot: true, hasRouteStub: true, rootHasContent: true }).reason).toContain("TODO(E-01:WS-S W-02)");
  });
});
