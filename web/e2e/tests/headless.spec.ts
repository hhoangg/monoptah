// The headless desktop binary (crates/monocode-core, bin web_link_headless, built by WS-D1).
import { spawnSync } from "node:child_process";
import { existsSync } from "node:fs";
import { test, expect } from "../lib/fixtures.ts";
import { headlessBin } from "../lib/probes.ts";
import { vectorsPath } from "../lib/paths.ts";

test.describe("headless desktop binary (E-01)", () => {
  test("check-vectors validates web/protocol/vectors.json", () => {
    const bin = headlessBin();
    if (!existsSync(bin)) console.log(`skip reason: headless binary not found at ${bin} (build web_link_headless)`);
    test.skip(!existsSync(bin), `headless binary not found at ${bin} (build web_link_headless or set E2E_HEADLESS_BIN)`);
    const r = spawnSync(bin, ["check-vectors", vectorsPath], { encoding: "utf8", timeout: 60_000 });
    expect({ status: r.status, stderr: (r.stderr ?? "").slice(-500) }).toEqual({ status: 0, stderr: expect.any(String) });
  });
});
