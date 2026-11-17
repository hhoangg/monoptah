// Records which Node globals workerd exposes in the vitest pool under ../wrangler.jsonc. The pool itself turns nodejs_compat on,
// so this test cannot prove "no flags"; the wrangler dev control run in FINDINGS.md does.
import { describe, it } from "vitest";

describe("runtime probe", () => {
  it("logs Node compat globals", () => {
    const g = globalThis as Record<string, unknown>;
    console.log(`S05-PROBE-2026 ${JSON.stringify({ Buffer: typeof g["Buffer"], process: typeof g["process"] })}`);
  });
});
