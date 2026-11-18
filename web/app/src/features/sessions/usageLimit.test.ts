// Copied from src/features/sessions/model/usageLimit.test.ts at ca73ac2b
import type { UsageLimit } from "@monoptah/protocol";
import { describe, expect, it } from "vitest";
import { formatResetDuration, formatUsageLimitReset, usageLimitWaiting, validateUsageLimit } from "./usageLimit";

describe("formatUsageLimitReset", () => {
  it("shows the time and what is left", () => {
    const now = new Date(2026, 8, 25, 22, 34).getTime();
    const today = new Date(2026, 8, 25, 23, 50).getTime();
    expect(formatUsageLimitReset(today, now)).toMatch(/ · in 1h 16m$/);
    const tomorrow = new Date(2026, 8, 26, 3, 16).getTime();
    expect(formatUsageLimitReset(tomorrow, now)).toMatch(/26.* · in 4h 42m$/);
  });
});

describe("formatResetDuration", () => {
  it("floors to whole units and says now once reset", () => {
    expect(formatResetDuration(0)).toBe("now");
    expect(formatResetDuration(-5)).toBe("now");
    expect(formatResetDuration(47 * 60_000 + 59_000)).toBe("47m");
    expect(formatResetDuration(3 * 3_600_000 + 54 * 60_000)).toBe("3h 54m");
    expect(formatResetDuration(2 * 3_600_000)).toBe("2h");
    expect(formatResetDuration(6 * 86_400_000 + 7 * 3_600_000)).toBe("6d 7h");
    expect(formatResetDuration(2 * 86_400_000)).toBe("2d");
  });
});

function state(patch: Partial<UsageLimit> = {}): UsageLimit {
  return {
    token: "t1",
    resetsAt: 10_000,
    resumeAtReset: false,
    canResume: false,
    canAutoResume: true,
    resetLabel: "Resets later",
    limitation: "Needs the desktop running.",
    ...patch,
  };
}

describe("usageLimitWaiting", () => {
  it("is true only while the reset is ahead", () => {
    expect(usageLimitWaiting({ resetsAt: 10_000 }, 5_000)).toBe(true);
    expect(usageLimitWaiting({ resetsAt: 10_000 }, 10_000)).toBe(false);
    expect(usageLimitWaiting({ resetsAt: null }, 0)).toBe(false);
    expect(usageLimitWaiting({}, 0)).toBe(false);
  });
});

describe("validateUsageLimit", () => {
  it("returns the state when the token matches and the action is allowed", () => {
    const limit = state();
    expect(validateUsageLimit(limit, "t1", true, 5_000)).toBe(limit);
    expect(validateUsageLimit(state({ canResume: true }), "t1", false, 5_000).canResume).toBe(true);
  });

  it("refuses when the session is no longer limited", () => {
    expect(() => validateUsageLimit(null, "t1", false)).toThrow("no longer paused");
    expect(() => validateUsageLimit(undefined, "t1", false)).toThrow("no longer paused");
  });

  it("refuses a stale token", () => {
    expect(() => validateUsageLimit(state(), "old", true)).toThrow("This session changed");
  });

  it("explains a resume attempted before the reset", () => {
    expect(() => validateUsageLimit(state(), "t1", false, 5_000)).toThrow(
      "Resets later. Enable Resume at reset or try again after the allowance resets.",
    );
  });

  it("refuses an automatic resume the desktop cannot honour", () => {
    expect(() => validateUsageLimit(state({ canAutoResume: false }), "t1", true, 5_000)).toThrow(
      "This session cannot continue right now",
    );
  });
});
