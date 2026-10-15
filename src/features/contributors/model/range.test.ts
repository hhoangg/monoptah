import { describe, expect, it } from "vitest";
import {
  customRange,
  earliestCustomDay,
  formatDay,
  parseDay,
  parseRange,
  rangeBounds,
  rangeDays,
  rangeKeyOf,
  rangeLabel,
  seasonChoices,
  serializeRange,
  sinceFor,
  toCustom,
  withEnd,
  withStart,
} from "./range";
import type { CustomRange } from "./range";
import { previousSeason, seasonById, seasonOf } from "./season";

const NOW = new Date(2026, 9, 15, 14, 30); // Thu Oct 15 2026, local
const day = (y: number, m: number, d: number) => new Date(y, m - 1, d);
const custom = (start: string, end: string): CustomRange => ({
  key: "custom",
  start,
  end,
});

describe("parseDay and formatDay", () => {
  it("round-trips a local day and rejects anything else", () => {
    expect(formatDay(day(2026, 3, 5))).toBe("2026-03-05");
    expect(parseDay("2026-03-05")).toEqual(day(2026, 3, 5));
    expect(parseDay("2026-02-31")).toBeUndefined();
    expect(parseDay("2026-3-5")).toBeUndefined();
    expect(parseDay("")).toBeUndefined();
    expect(parseDay("2026-03-05T00:00:00")).toBeUndefined();
  });
});

describe("customRange", () => {
  it("keeps a valid range as it is", () => {
    expect(customRange(day(2026, 10, 1), day(2026, 10, 15), NOW)).toEqual(
      custom("2026-10-01", "2026-10-15"),
    );
  });

  it("allows exactly 30 days, both ends counted", () => {
    expect(customRange(day(2026, 9, 16), day(2026, 10, 15), NOW)).toEqual(
      custom("2026-09-16", "2026-10-15"),
    );
  });

  it("keeps the end and moves the start when the span is longer than 30 days", () => {
    expect(customRange(day(2026, 9, 15), day(2026, 10, 15), NOW)).toEqual(
      custom("2026-09-16", "2026-10-15"),
    );
    expect(customRange(day(2020, 1, 1), day(2026, 10, 10), NOW)).toEqual(
      custom("2026-09-11", "2026-10-10"),
    );
  });

  it("does not end after today", () => {
    expect(customRange(day(2026, 10, 10), day(2026, 11, 20), NOW)).toEqual(
      custom("2026-10-10", "2026-10-15"),
    );
    expect(customRange(day(2026, 12, 1), day(2026, 12, 5), NOW)).toEqual(
      custom("2026-10-15", "2026-10-15"),
    );
  });

  it("does not start after it ends", () => {
    expect(customRange(day(2026, 10, 12), day(2026, 10, 5), NOW)).toEqual(
      custom("2026-10-05", "2026-10-05"),
    );
  });
});

describe("earliestCustomDay", () => {
  it("is the start of the season before the oldest selectable one", () => {
    // Oct 2026 offers Oct..May, so the horizon is Apr 1.
    expect(earliestCustomDay(NOW)).toEqual(new Date(2026, 3, 1));
    // Crosses the year boundary.
    expect(earliestCustomDay(new Date(2027, 1, 3))).toEqual(
      new Date(2026, 7, 1),
    );
  });

  it("is the same horizon season selection collects back to", () => {
    const choices = seasonChoices(NOW);
    expect(earliestCustomDay(NOW)).toEqual(
      previousSeason(choices[choices.length - 1]).start,
    );
  });
});

describe("customRange horizon", () => {
  it("moves a start before the horizon up to it", () => {
    expect(customRange(day(2026, 1, 1), day(2026, 4, 10), NOW)).toEqual(
      custom("2026-04-01", "2026-04-10"),
    );
  });

  it("allows a range that starts on the horizon", () => {
    expect(customRange(day(2026, 4, 1), day(2026, 4, 30), NOW)).toEqual(
      custom("2026-04-01", "2026-04-30"),
    );
  });

  it("moves an end before the horizon up to it, as one day", () => {
    expect(customRange(day(2020, 1, 1), day(2020, 1, 10), NOW)).toEqual(
      custom("2026-04-01", "2026-04-01"),
    );
  });
});

describe("withStart and withEnd", () => {
  const base = custom("2026-10-01", "2026-10-15");

  it("changes one end and leaves the other when the span stays valid", () => {
    expect(withStart(base, day(2026, 10, 5), NOW)).toEqual(
      custom("2026-10-05", "2026-10-15"),
    );
    expect(withEnd(base, day(2026, 10, 10), NOW)).toEqual(
      custom("2026-10-01", "2026-10-10"),
    );
  });

  it("drags the end along when the start would make the span longer than 30 days", () => {
    expect(withStart(base, day(2026, 8, 1), NOW)).toEqual(
      custom("2026-08-01", "2026-08-30"),
    );
  });

  it("drags the start along when the end would make the span longer than 30 days", () => {
    expect(withEnd(base, day(2026, 10, 15), NOW)).toEqual(base);
    const early = custom("2026-09-01", "2026-09-10");
    expect(withEnd(early, day(2026, 10, 14), NOW)).toEqual(
      custom("2026-09-15", "2026-10-14"),
    );
  });

  it("moves the other end when the pick crosses it", () => {
    expect(withStart(base, day(2026, 10, 15), NOW)).toEqual(
      custom("2026-10-15", "2026-10-15"),
    );
    expect(withEnd(base, day(2026, 9, 20), NOW)).toEqual(
      custom("2026-09-20", "2026-09-20"),
    );
  });

  it("stops at the horizon when the pick is older", () => {
    expect(withStart(base, day(2020, 1, 1), NOW)).toEqual(
      custom("2026-04-01", "2026-04-30"),
    );
    expect(withEnd(base, day(2020, 1, 1), NOW)).toEqual(
      custom("2026-04-01", "2026-04-01"),
    );
  });

  it("never lets the end pass today", () => {
    expect(withStart(base, day(2026, 10, 14), NOW).end).toBe("2026-10-15");
    expect(withEnd(base, day(2026, 12, 1), NOW).end).toBe("2026-10-15");
  });
});

describe("parseRange", () => {
  it("accepts the two presets", () => {
    expect(parseRange("7d", NOW)).toBe("7d");
    expect(parseRange("30d", NOW)).toBe("30d");
  });

  it("rejects the removed presets and anything else", () => {
    for (const value of ["90d", "1y", "all", "14d", "", "custom", "forever"]) {
      expect(parseRange(value, NOW)).toBeUndefined();
    }
    expect(parseRange(null, NOW)).toBeUndefined();
    expect(parseRange(undefined, NOW)).toBeUndefined();
  });

  it("restores a valid custom range", () => {
    expect(parseRange("custom:2026-10-01:2026-10-15", NOW)).toEqual(
      custom("2026-10-01", "2026-10-15"),
    );
  });

  it("clamps a custom range that is too long or ends after today", () => {
    expect(parseRange("custom:2026-01-01:2026-10-15", NOW)).toEqual(
      custom("2026-09-16", "2026-10-15"),
    );
    expect(parseRange("custom:2026-10-10:2027-01-01", NOW)).toEqual(
      custom("2026-10-10", "2026-10-15"),
    );
  });

  it("clamps a stored custom range older than the horizon", () => {
    expect(parseRange("custom:2020-01-01:2020-01-10", NOW)).toEqual(
      custom("2026-04-01", "2026-04-01"),
    );
    expect(parseRange("custom:2026-03-20:2026-04-10", NOW)).toEqual(
      custom("2026-04-01", "2026-04-10"),
    );
  });

  it("rejects a malformed custom range", () => {
    for (const value of [
      "custom:2026-10-01",
      "custom:2026-10-01:",
      "custom:2026-02-31:2026-03-01",
      "custom:2026-10-01:2026-10-15:x",
      "Custom:2026-10-01:2026-10-15",
    ]) {
      expect(parseRange(value, NOW)).toBeUndefined();
    }
  });

  it("round-trips through serializeRange", () => {
    for (const range of [
      "7d",
      "30d",
      custom("2026-10-01", "2026-10-15"),
    ] as const) {
      expect(parseRange(serializeRange(range), NOW)).toEqual(range);
    }
  });
});

describe("rangeKeyOf", () => {
  it("is the preset or custom", () => {
    expect(rangeKeyOf("7d")).toBe("7d");
    expect(rangeKeyOf(custom("2026-10-01", "2026-10-02"))).toBe("custom");
  });
});

describe("rangeBounds", () => {
  it("covers N whole local days ending after today", () => {
    const { start, end } = rangeBounds("7d", NOW);
    expect(end).toEqual(new Date(2026, 9, 16));
    expect(start).toEqual(new Date(2026, 9, 9));
  });

  it("uses 30 days for 30d", () => {
    expect(rangeDays("30d", NOW)).toBe(30);
    expect(rangeDays("7d", NOW)).toBe(7);
  });

  it("covers a custom range from its first day to the end of its last", () => {
    const { start, end } = rangeBounds(custom("2026-10-01", "2026-10-15"), NOW);
    expect(start).toEqual(new Date(2026, 9, 1));
    expect(end).toEqual(new Date(2026, 9, 16));
    expect(rangeDays(custom("2026-10-01", "2026-10-15"), NOW)).toBe(15);
    expect(rangeDays(custom("2026-10-15", "2026-10-15"), NOW)).toBe(1);
  });

  it("clamps an out-of-range custom value to 30 days ending no later than today", () => {
    const long = rangeBounds(custom("2025-01-01", "2026-10-10"), NOW);
    expect(long.start).toEqual(new Date(2026, 8, 11));
    expect(long.end).toEqual(new Date(2026, 9, 11));
    const future = rangeBounds(custom("2026-10-10", "2026-12-31"), NOW);
    expect(future.end).toEqual(new Date(2026, 9, 16));
  });

  it("clamps again when the clock moves, so a range never extends past today", () => {
    const earlier = new Date(2026, 9, 12, 9);
    const { end } = rangeBounds(custom("2026-10-10", "2026-10-15"), earlier);
    expect(end).toEqual(new Date(2026, 9, 13));
  });

  it("spans a daylight saving change as whole local days", () => {
    const { start, end } = rangeBounds(
      custom("2026-03-01", "2026-03-30"),
      new Date(2026, 2, 30, 12),
    );
    expect(start).toEqual(new Date(2026, 2, 1));
    expect(end).toEqual(new Date(2026, 2, 31));
  });
});

describe("toCustom", () => {
  it("starts from the days a preset shows", () => {
    expect(toCustom("7d", NOW)).toEqual(custom("2026-10-09", "2026-10-15"));
    expect(toCustom("30d", NOW)).toEqual(custom("2026-09-16", "2026-10-15"));
  });

  it("returns a custom range as it is", () => {
    const range = custom("2026-10-01", "2026-10-05");
    expect(toCustom(range, NOW)).toEqual(range);
  });
});

describe("rangeLabel", () => {
  it("is the preset key or the two days", () => {
    expect(rangeLabel("30d", NOW)).toBe("30d");
    expect(rangeLabel(custom("2026-10-01", "2026-10-15"), NOW)).toBe(
      "2026-10-01 to 2026-10-15",
    );
  });
});

const iso = (d: Date) => d.toISOString().replace(".000Z", "Z");
const previousStart = (range: Parameters<typeof rangeBounds>[0], now: Date) => {
  const { start, end } = rangeBounds(range, now);
  return new Date(start.getTime() - (end.getTime() - start.getTime()));
};

describe("sinceFor", () => {
  it("reaches back to the previous season start when that is earlier than the previous period", () => {
    expect(sinceFor("7d", NOW)).toBe(iso(new Date(2026, 8, 1)));
  });

  it("covers the whole previous period for 30d", () => {
    expect(sinceFor("30d", NOW)).toBe(iso(previousStart("30d", NOW)));
  });

  it("covers the previous period on every day of a month for 7d and 30d", () => {
    for (const range of ["7d", "30d"] as const) {
      for (let day = 1; day <= 31; day += 1) {
        const now = new Date(2026, 9, day, 12);
        expect(Date.parse(sinceFor(range, now))).toBeLessThanOrEqual(
          previousStart(range, now).getTime(),
        );
      }
    }
  });

  it("covers the equal-length period before a custom range", () => {
    const long = custom("2026-09-16", "2026-10-15"); // 30 days
    expect(sinceFor(long, NOW)).toBe(iso(new Date(2026, 7, 17)));
    const short = custom("2026-10-06", "2026-10-15"); // 10 days
    // The previous period would start Sep 26; the previous season (Sep 1) is earlier.
    expect(sinceFor(short, NOW)).toBe(iso(new Date(2026, 8, 1)));
  });

  it("reaches back before a custom range in the past, whatever the current season", () => {
    const range = custom("2026-05-10", "2026-05-19"); // 10 days
    expect(sinceFor(range, NOW)).toBe(iso(new Date(2026, 3, 30))); // May 10 minus 10 days
  });

  it("is the same for every moment of one day", () => {
    expect(sinceFor("30d", new Date(2026, 9, 15, 0, 1))).toBe(
      sinceFor("30d", new Date(2026, 9, 15, 23, 59)),
    );
  });

  it("never carries milliseconds", () => {
    expect(sinceFor("30d", NOW)).not.toContain(".");
  });
});

describe("sinceFor with a selected season", () => {
  it("is unchanged for the current season", () => {
    for (const range of [
      "7d",
      "30d",
      custom("2026-10-01", "2026-10-15"),
    ] as const) {
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
    const range = custom("2026-06-10", "2026-07-09");
    expect(sinceFor(range, NOW, sept)).toBe(sinceFor(range, NOW));
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
