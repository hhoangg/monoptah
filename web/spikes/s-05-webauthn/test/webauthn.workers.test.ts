// S-05 in workerd: compatibility_date 2026-09-01, no compatibility flags (see ../wrangler.jsonc).
import { describe, expect, it } from "vitest";
import { roundTrip, timeVerify } from "../src/ceremony.ts";
import type { Algorithm } from "../src/authenticator.ts";

const ALGORITHMS: Algorithm[] = ["ES256", "RS256", "Ed25519"];

describe("@simplewebauthn/server in workerd without compatibility flags", () => {
  it.each(ALGORITHMS)("registers and authenticates with %s", async (alg) => {
    const r = await roundTrip(alg, () => performance.now());
    expect(r.registered).toBe(true);
    expect(r.verified).toBe(true);
    expect(r.newCounter).toBe(1);
    expect(r.replayError).toMatch(/counter/i);
    expect(r.lowerCounterError).toMatch(/counter/i);
    expect(r.zeroCounterAccepted).toBe(true);
    console.log(`S05-WORKERD ${alg} ${JSON.stringify(r)}`);
  });

  it("records verify timing in workerd (timers only advance on I/O, so this may read 0)", async () => {
    const out: Record<string, unknown> = {};
    for (const alg of ALGORITHMS) out[alg] = await timeVerify(alg, 30, () => performance.now());
    out["dateNow"] = Date.now();
    console.log(`S05-WORKERD-TIMING ${JSON.stringify(out)}`);
    expect(Object.keys(out).length).toBeGreaterThan(0);
  });
});
