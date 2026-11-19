import { describe, expect, it } from "vitest";
import { chatListTime, relativeTime } from "./relativeTime";

const NOW = new Date(2026, 4, 20, 15, 0, 0).getTime();

describe("relativeTime", () => {
  it("steps through now, minutes, hours and days", () => {
    expect(relativeTime(NOW - 5_000, NOW)).toBe("now");
    expect(relativeTime(NOW - 5 * 60_000, NOW)).toBe("5m");
    expect(relativeTime(NOW - 3 * 3_600_000, NOW)).toBe("3h");
    expect(relativeTime(NOW - 2 * 86_400_000, NOW)).toBe("2d");
    expect(relativeTime(NOW + 10_000, NOW)).toBe("now");
  });
});

describe("chatListTime", () => {
  it("says Yesterday for the day before and a short date after that", () => {
    expect(chatListTime(new Date(2026, 4, 19, 9, 0).getTime(), NOW)).toBe("Yesterday");
    expect(chatListTime(new Date(2026, 4, 1, 9, 0).getTime(), NOW)).toMatch(/\d/);
  });
  it("shows a time for today", () => {
    expect(chatListTime(NOW - 3_600_000, NOW)).toMatch(/\d/);
  });
});
