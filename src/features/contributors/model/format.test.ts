import { describe, expect, it } from "vitest";
import { formatDelta, formatScore, relativeTime } from "./format";

describe("formatScore", () => {
  it("rounds and groups thousands", () => {
    expect(formatScore(612.4)).toBe("612");
    expect(formatScore(612.5)).toBe("613");
    expect(formatScore(1234.6)).toBe("1,235");
    expect(formatScore(1_234_567)).toBe("1,234,567");
    expect(formatScore(0)).toBe("0");
  });

  it("never prints a negative zero", () => {
    expect(formatScore(-0.2)).toBe("0");
  });
});

describe("formatDelta", () => {
  it("prints a signed percentage", () => {
    expect(formatDelta(0.18)).toBe("+18%");
    expect(formatDelta(-0.06)).toBe("-6%");
    expect(formatDelta(1.5)).toBe("+150%");
  });

  it("prints 0% without a sign", () => {
    expect(formatDelta(0)).toBe("0%");
    expect(formatDelta(0.004)).toBe("0%");
    expect(formatDelta(-0.004)).toBe("0%");
  });
});

describe("relativeTime", () => {
  const now = new Date("2026-10-15T12:00:00Z");
  const ago = (ms: number) => new Date(now.getTime() - ms).toISOString();
  const MIN = 60_000;
  const HOUR = 60 * MIN;
  const DAY = 24 * HOUR;

  it("says just now under a minute and for the future", () => {
    expect(relativeTime(ago(0), now)).toBe("just now");
    expect(relativeTime(ago(59_000), now)).toBe("just now");
    expect(relativeTime(ago(-5 * MIN), now)).toBe("just now");
  });

  it("uses minutes, hours and days", () => {
    expect(relativeTime(ago(5 * MIN), now)).toBe("5m ago");
    expect(relativeTime(ago(2 * HOUR + 10 * MIN), now)).toBe("2h ago");
    expect(relativeTime(ago(23 * HOUR), now)).toBe("23h ago");
    expect(relativeTime(ago(DAY), now)).toBe("1d ago");
    expect(relativeTime(ago(12 * DAY + 5 * HOUR), now)).toBe("12d ago");
  });

  it("falls back to months and years for old dates", () => {
    expect(relativeTime(ago(59 * DAY), now)).toBe("59d ago");
    expect(relativeTime(ago(90 * DAY), now)).toBe("3mo ago");
    expect(relativeTime(ago(400 * DAY), now)).toBe("1y ago");
  });

  it("returns an empty string for an invalid date", () => {
    expect(relativeTime("not a date", now)).toBe("");
  });
});
