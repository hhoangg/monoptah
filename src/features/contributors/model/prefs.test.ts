// @vitest-environment happy-dom
import { beforeEach, describe, expect, it } from "vitest";
import {
  MERGE_KEY,
  RANGE_KEY,
  parsePick,
  readMerge,
  readPeriodPick,
  serializePick,
  writeMerge,
  writePeriodPick,
} from "./prefs";

const NOW = new Date(2026, 9, 15, 12);

beforeEach(() => {
  localStorage.clear();
});

describe("contributors prefs", () => {
  it("defaults to 7d and merging on", () => {
    expect(readPeriodPick(NOW)).toEqual({ kind: "7d", id: null });
    expect(readMerge()).toBe(true);
  });

  it("uses the monocode prefix keys", () => {
    expect(RANGE_KEY).toBe("monocode.contributors.range");
    expect(MERGE_KEY).toBe("monocode.contributors.mergeIdentities");
  });

  it("stores a kind that follows the current period as the bare kind", () => {
    for (const kind of ["7d", "month", "quarter", "year"] as const) {
      writePeriodPick({ kind, id: null });
      expect(localStorage.getItem(RANGE_KEY)).toBe(kind);
      expect(readPeriodPick(NOW)).toEqual({ kind, id: null });
    }
  });

  it("stores a pinned period as kind:id and reads it back", () => {
    const cases = [
      { kind: "month", id: "2026-08", text: "month:2026-08" },
      { kind: "quarter", id: "2026-Q2", text: "quarter:2026-Q2" },
      { kind: "year", id: "2025", text: "year:2025" },
    ] as const;
    for (const { kind, id, text } of cases) {
      writePeriodPick({ kind, id });
      expect(localStorage.getItem(RANGE_KEY)).toBe(text);
      expect(readPeriodPick(NOW)).toEqual({ kind, id });
    }
  });

  it("falls back to 7d for the removed ranges", () => {
    for (const removed of [
      "30d",
      "90d",
      "1y",
      "all",
      "custom",
      "custom:2026-10-01:2026-10-10",
    ]) {
      localStorage.setItem(RANGE_KEY, removed);
      expect(readPeriodPick(NOW)).toEqual({ kind: "7d", id: null });
    }
  });

  it("falls back to 7d for garbage and unusable ids", () => {
    for (const bad of [
      "forever",
      "",
      ":",
      "month:",
      "month:2026-13",
      "month:2026-Q3",
      "quarter:2026-Q9",
      "year:soon",
      "7d:7d",
      "7d:x",
      "month:2027-01",
      "Month",
    ]) {
      localStorage.setItem(RANGE_KEY, bad);
      expect(readPeriodPick(NOW)).toEqual({ kind: "7d", id: null });
    }
  });

  it("serializes and parses symmetrically", () => {
    const pick = { kind: "quarter", id: "2026-Q1" } as const;
    expect(parsePick(serializePick(pick), NOW)).toEqual(pick);
    expect(parsePick(null, NOW)).toBeUndefined();
    expect(parsePick(undefined, NOW)).toBeUndefined();
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
