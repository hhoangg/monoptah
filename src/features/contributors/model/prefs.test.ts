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

  it("round-trips the range and falls back on garbage", () => {
    writeRange("90d");
    expect(localStorage.getItem(RANGE_KEY)).toBe("90d");
    expect(readRange()).toBe("90d");
    localStorage.setItem(RANGE_KEY, "forever");
    expect(readRange()).toBe("30d");
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
