// @vitest-environment happy-dom
import { beforeEach, describe, expect, it } from "vitest";
import {
  MERGE_KEY,
  RANGE_KEY,
  readMerge,
  readRange,
  writeMerge,
  writeRange,
} from "./prefs";

beforeEach(() => {
  localStorage.clear();
});

describe("contributors prefs", () => {
  it("defaults to 30d and merging on", () => {
    expect(readRange()).toBe("30d");
    expect(readMerge()).toBe(true);
  });

  it("uses the monocode prefix keys", () => {
    expect(RANGE_KEY).toBe("monocode.contributors.range");
    expect(MERGE_KEY).toBe("monocode.contributors.mergeIdentities");
  });

  it("round-trips the presets and falls back on garbage", () => {
    writeRange("7d");
    expect(localStorage.getItem(RANGE_KEY)).toBe("7d");
    expect(readRange()).toBe("7d");
    localStorage.setItem(RANGE_KEY, "forever");
    expect(readRange()).toBe("30d");
  });

  it("falls back to 30d for the removed presets", () => {
    for (const removed of ["90d", "1y", "all"]) {
      localStorage.setItem(RANGE_KEY, removed);
      expect(readRange()).toBe("30d");
    }
  });

  it("round-trips a custom range", () => {
    const now = new Date(2026, 9, 15, 12);
    const range = {
      key: "custom",
      start: "2026-10-01",
      end: "2026-10-10",
    } as const;
    writeRange(range);
    expect(localStorage.getItem(RANGE_KEY)).toBe(
      "custom:2026-10-01:2026-10-10",
    );
    expect(readRange(now)).toEqual(range);
  });

  it("clamps a stored custom range that is too long and drops a broken one", () => {
    const now = new Date(2026, 9, 15, 12);
    localStorage.setItem(RANGE_KEY, "custom:2026-01-01:2026-10-15");
    expect(readRange(now)).toEqual({
      key: "custom",
      start: "2026-09-16",
      end: "2026-10-15",
    });
    localStorage.setItem(RANGE_KEY, "custom:soon:later");
    expect(readRange(now)).toBe("30d");
  });

  it("round-trips the merge toggle", () => {
    writeMerge(false);
    expect(readMerge()).toBe(false);
    writeMerge(true);
    expect(readMerge()).toBe(true);
    localStorage.setItem(MERGE_KEY, "maybe");
    expect(readMerge()).toBe(true);
  });
});
