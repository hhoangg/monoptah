import { describe, expect, it } from "vitest";
import {
  ALL_SINCE,
  parseRange,
  rangeBounds,
  seasonChoices,
  sinceFor,
} from "./range";
import { seasonById, seasonOf } from "./season";

const NOW = new Date(2026, 9, 15, 14, 30); // Thu Oct 15 2026, local

describe("parseRange", () => {
  it("accepts the five keys and rejects anything else", () => {
    for (const key of ["7d", "30d", "90d", "1y", "all"]) {
      expect(parseRange(key)).toBe(key);
    }
    expect(parseRange("14d")).toBeUndefined();
    expect(parseRange("")).toBeUndefined();
    expect(parseRange(null)).toBeUndefined();
  });
});

describe("rangeBounds", () => {
  it("covers N whole local days ending after today", () => {
    const { start, end } = rangeBounds("7d", NOW);
    expect(end).toEqual(new Date(2026, 9, 16));
    expect(start).toEqual(new Date(2026, 9, 9));
  });

  it("uses 30, 90 and 365 days for the other fixed ranges", () => {
    const day = 24 * 3600 * 1000;
    for (const [key, days] of [
      ["30d", 30],
      ["90d", 90],
      ["1y", 365],
    ] as const) {
      const { start, end } = rangeBounds(key, NOW);
      expect(Math.round((end.getTime() - start.getTime()) / day)).toBe(days);
    }
  });

  it("starts 'all' at the earliest commit day, or 30 days back without one", () => {
    const earliest = new Date(2026, 2, 5, 9, 41);
    expect(rangeBounds("all", NOW, earliest).start).toEqual(
      new Date(2026, 2, 5),
    );
    expect(rangeBounds("all", NOW).start).toEqual(new Date(2026, 8, 16));
  });
});

const iso = (d: Date) => d.toISOString().replace(".000Z", "Z");
const previousStart = (range: "7d" | "30d" | "90d" | "1y", now: Date) => {
  const { start, end } = rangeBounds(range, now);
  return new Date(start.getTime() - (end.getTime() - start.getTime()));
};

describe("sinceFor", () => {
  it("reaches back to the previous season start when that is earlier than the previous period", () => {
    expect(sinceFor("7d", NOW)).toBe(iso(new Date(2026, 8, 1)));
  });

  it("covers the whole previous period for 30d and 90d", () => {
    expect(sinceFor("30d", NOW)).toBe(iso(previousStart("30d", NOW)));
    expect(sinceFor("90d", NOW)).toBe(iso(previousStart("90d", NOW)));
  });

  it("covers the previous period on every day of a month for 7d, 30d and 90d", () => {
    for (const range of ["7d", "30d", "90d"] as const) {
      for (let day = 1; day <= 31; day += 1) {
        const now = new Date(2026, 9, day, 12);
        expect(Date.parse(sinceFor(range, now))).toBeLessThanOrEqual(
          previousStart(range, now).getTime(),
        );
      }
    }
  });

  it("only reaches back to the range start for 1y, so its deltas stay hidden", () => {
    expect(sinceFor("1y", NOW)).toBe(iso(rangeBounds("1y", NOW).start));
    expect(Date.parse(sinceFor("1y", NOW))).toBeGreaterThan(
      previousStart("1y", NOW).getTime(),
    );
  });

  it("is the same for every moment of one day", () => {
    expect(sinceFor("90d", new Date(2026, 9, 15, 0, 1))).toBe(
      sinceFor("90d", new Date(2026, 9, 15, 23, 59)),
    );
  });

  it("uses a fixed old date for 'all'", () => {
    expect(sinceFor("all", NOW)).toBe(ALL_SINCE);
  });

  it("never carries milliseconds", () => {
    expect(sinceFor("1y", NOW)).not.toContain(".");
  });
});

describe("sinceFor with a selected season", () => {
  it("is unchanged for the current season", () => {
    for (const range of ["7d", "30d", "90d", "1y", "all"] as const) {
      expect(sinceFor(range, NOW, seasonOf(NOW))).toBe(sinceFor(range, NOW));
    }
  });

  it("reaches back to the start of the season before the selected one", () => {
    const july = seasonById("2026-07");
    expect(july).not.toBeNull();
    expect(sinceFor("30d", NOW, july!)).toBe(iso(new Date(2026, 5, 1)));
  });

  it("keeps the range coverage when the selected season is already covered", () => {
    const sept = seasonById("2026-09")!;
    expect(sinceFor("90d", NOW, sept)).toBe(sinceFor("90d", NOW));
  });

  it("still uses the fixed old date for 'all'", () => {
    expect(sinceFor("all", NOW, seasonById("2026-05")!)).toBe(ALL_SINCE);
  });
});

describe("seasonChoices", () => {
  it("lists the current month and the five before it, newest first", () => {
    expect(seasonChoices(NOW).map((s) => s.id)).toEqual([
      "2026-10",
      "2026-09",
      "2026-08",
      "2026-07",
      "2026-06",
      "2026-05",
    ]);
  });

  it("crosses a year boundary", () => {
    const ids = seasonChoices(new Date(2027, 1, 3)).map((s) => s.id);
    expect(ids).toEqual([
      "2027-02",
      "2027-01",
      "2026-12",
      "2026-11",
      "2026-10",
      "2026-09",
    ]);
  });

  it("carries the display label", () => {
    expect(seasonChoices(NOW)[0].label).toBe("Oct 2026");
  });
});
