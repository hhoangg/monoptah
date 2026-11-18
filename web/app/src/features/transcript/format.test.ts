import { describe, expect, it } from "vitest";
import { dayStamp, formatElapsed, formatMetricCount, formatWorkingDuration, groupDigits, hasTurnMetrics, turnMetricsSheet } from "./format";

describe("formatWorkingDuration", () => {
  it("reads like AgentTranscript", () => {
    expect(formatWorkingDuration(null)).toBe("Working...");
    expect(formatWorkingDuration(null, "Claude Opus")).toBe("Claude Opus working...");
    expect(formatWorkingDuration(124_000, "Claude Opus", true)).toBe("Claude Opus worked for 2m 4s");
    expect(formatWorkingDuration(72_000, undefined, true)).toBe("Worked for 1m 12s");
    expect(formatWorkingDuration(null, "Claude", true)).toBe("Claude worked");
  });
  it("formats elapsed time", () => {
    expect(formatElapsed(400)).toBe("1s");
    expect(formatElapsed(59_000)).toBe("59s");
    expect(formatElapsed(120_000)).toBe("2m");
    expect(formatElapsed(null)).toBeNull();
  });
});

describe("dayStamp", () => {
  const now = new Date(2026, 5, 15, 12, 0).getTime();
  it("names today and yesterday", () => {
    expect(dayStamp(new Date(2026, 5, 15, 9, 30).getTime(), now)[0]).toBe("Today");
    expect(dayStamp(new Date(2026, 5, 14, 9, 30).getTime(), now)[0]).toBe("Yesterday");
  });
  it("adds the year for another year", () => {
    expect(dayStamp(new Date(2024, 0, 3, 9, 30).getTime(), now)[0]).toContain("2024");
    expect(dayStamp(new Date(2026, 0, 3, 9, 30).getTime(), now)[0]).not.toContain("2026");
  });
});

describe("metrics", () => {
  it("knows when there is nothing to show", () => {
    expect(hasTurnMetrics(undefined)).toBe(false);
    expect(hasTurnMetrics({ inputTokens: 0 })).toBe(false);
    expect(hasTurnMetrics({ cacheHitPercent: 0 })).toBe(true);
  });
  it("builds highlights and token rows", () => {
    const sheet = turnMetricsSheet({ inputTokens: 1234, outputTokens: 500, cacheHitPercent: 87.4 }, 10_000);
    expect(sheet.highlights).toEqual([
      { label: "Cache hit", value: "87", unit: "%" },
      { label: "Output speed", value: "50", unit: "tok/s" },
      { label: "Duration", value: "10s" },
    ]);
    expect(sheet.tokens).toEqual([
      { label: "Input", value: "1,234" },
      { label: "Output", value: "500" },
    ]);
  });
  it("groups and compacts counts", () => {
    expect(groupDigits(1234567)).toBe("1,234,567");
    expect(formatMetricCount(950)).toBe("950");
    expect(formatMetricCount(12_300)).toBe("12.3K");
    expect(formatMetricCount(1_000_000)).toBe("1M");
  });
});
