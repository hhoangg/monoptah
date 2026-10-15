import { describe, expect, it } from "vitest";
import { formatCompact } from "./compact";

describe("formatCompact", () => {
  it("keeps small numbers whole", () => {
    expect(formatCompact(0)).toBe("0");
    expect(formatCompact(412)).toBe("412");
    expect(formatCompact(999)).toBe("999");
  });

  it("abbreviates thousands with one decimal, dropping a trailing zero", () => {
    expect(formatCompact(1000)).toBe("1k");
    expect(formatCompact(1234)).toBe("1.2k");
    expect(formatCompact(38_200)).toBe("38.2k");
    expect(formatCompact(12_949)).toBe("12.9k");
  });

  it("rolls over to millions instead of printing 1000k", () => {
    expect(formatCompact(999_999)).toBe("1M");
    expect(formatCompact(2_500_000)).toBe("2.5M");
  });

  it("never prints a negative zero or NaN", () => {
    expect(formatCompact(-0)).toBe("0");
    expect(formatCompact(Number.NaN)).toBe("0");
  });
});
