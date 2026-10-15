import { describe, expect, it } from "vitest";
import { axisTicks } from "./axis";

describe("axisTicks", () => {
  it("is empty for nothing and one tick for one bucket", () => {
    expect(axisTicks(0)).toEqual([]);
    expect(axisTicks(1)).toEqual([0]);
  });

  it("labels every bucket when there are few", () => {
    expect(axisTicks(5)).toEqual([0, 1, 2, 3, 4]);
    expect(axisTicks(8)).toEqual([0, 1, 2, 3, 4, 5, 6, 7]);
  });

  it("always includes the first and the last bucket", () => {
    for (const n of [2, 9, 13, 24, 53, 120, 521]) {
      const ticks = axisTicks(n);
      expect(ticks[0]).toBe(0);
      expect(ticks[ticks.length - 1]).toBe(n - 1);
    }
  });

  it("keeps ticks increasing, bounded in number and never crowded", () => {
    for (const n of [9, 13, 14, 24, 53, 120, 521]) {
      const ticks = axisTicks(n);
      const step = Math.ceil(n / 8);
      expect(ticks.length).toBeLessThanOrEqual(10);
      for (let i = 1; i < ticks.length; i += 1) {
        expect(ticks[i]).toBeGreaterThan(ticks[i - 1]);
        expect(ticks[i] - ticks[i - 1]).toBeGreaterThanOrEqual(
          Math.ceil(step / 2),
        );
      }
    }
  });

  it("never labels two neighbouring buckets", () => {
    for (let count = 9; count <= 60; count += 1) {
      const ticks = axisTicks(count);
      for (let i = 1; i < ticks.length; i += 1) {
        expect(ticks[i] - ticks[i - 1]).toBeGreaterThanOrEqual(2);
      }
    }
  });
});
