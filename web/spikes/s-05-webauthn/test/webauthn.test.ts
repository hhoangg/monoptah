// S-05 in Node (Node WebCrypto): same scenario as the workerd test, plus local timing with performance.now().
import { describe, expect, it } from "vitest";
import { roundTrip, timeVerify } from "../src/ceremony.ts";
import type { Algorithm } from "../src/authenticator.ts";

const ALGORITHMS: Algorithm[] = ["ES256", "RS256", "Ed25519"];

describe("@simplewebauthn/server in Node", () => {
  it.each(ALGORITHMS)("registers and authenticates with %s", async (alg) => {
    const r = await roundTrip(alg, () => performance.now());
    expect(r.registered).toBe(true);
    expect(r.verified).toBe(true);
    expect(r.newCounter).toBe(1);
    // A replayed or lower counter is rejected once a non-zero counter is stored.
    expect(r.replayError).toMatch(/counter/i);
    expect(r.lowerCounterError).toMatch(/counter/i);
    // Authenticators that never count (counter 0 and stored 0) are accepted.
    expect(r.zeroCounterAccepted).toBe(true);
  });

  it("measure: verify time locally (Node, not production CPU) stays far under the 10 ms free cap", async () => {
    const out: Record<string, unknown> = {};
    for (const alg of ALGORITHMS) {
      const t = await timeVerify(alg, 200, () => performance.now());
      out[alg] = t;
      expect(t.meanMs).toBeGreaterThan(0);
      expect(t.meanMs).toBeLessThan(10);
    }
    console.log(`S05-NODE-TIMING ${JSON.stringify(out)}`);
  });
});
