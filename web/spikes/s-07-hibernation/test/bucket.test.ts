import { describe, expect, it } from "vitest";
import { TokenBucket } from "../src/bucket.ts";

describe("TokenBucket", () => {
  it("allows a burst then refuses", () => {
    const b = new TokenBucket(3, 1, 0);
    expect([b.take(1, 0), b.take(1, 0), b.take(1, 0), b.take(1, 0)]).toEqual([true, true, true, false]);
  });

  it("refills with time and caps at capacity", () => {
    const b = new TokenBucket(3, 2, 0);
    b.take(3, 0);
    expect(b.take(1, 400)).toBe(false); // 0.8 tokens
    expect(b.take(1, 500)).toBe(true); // 1.0 token
    b.take(0, 100_000);
    expect(b.snapshot().tokens).toBe(3);
  });

  it("is rebuilt from a snapshot including the time spent hibernating", () => {
    const b = new TokenBucket(10, 1, 0);
    b.take(10, 0);
    const state = b.snapshot();
    const rebuilt = new TokenBucket(10, 1, 5000, state);
    expect(rebuilt.snapshot().tokens).toBeCloseTo(5, 5);
    expect(rebuilt.take(5, 5000)).toBe(true);
    expect(rebuilt.take(1, 5000)).toBe(false);
  });

  it("clamps a corrupt snapshot", () => {
    const b = new TokenBucket(5, 1, 0, { tokens: 999, at: 0 });
    expect(b.snapshot().tokens).toBe(5);
    expect(new TokenBucket(5, 1, 0, { tokens: -4, at: 0 }).snapshot().tokens).toBe(0);
  });
});
