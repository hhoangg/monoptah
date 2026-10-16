import { afterEach, describe, expect, it, onTestFinished } from "vitest";
import {
  DEFAULT_PERIOD_KIND,
  PERIOD_KINDS,
  currentPeriod,
  isInPeriod,
  isWeekend,
  localDayKey,
  nextPeriod,
  periodDayKeys,
  pickOf,
  resolvePick,
  periodFromId,
  previousComparableSpan,
  previousPeriod,
  syncWindow,
} from "./period";
import { priorOf } from "./testFixtures";

const NOW = new Date(2026, 8, 15, 14, 30); // Tue 2026-09-15

const ymd = (d: Date) => [d.getFullYear(), d.getMonth() + 1, d.getDate()];

describe("kinds", () => {
  it("lists the five kinds, All time last, and defaults to 7d", () => {
    expect(PERIOD_KINDS).toEqual([
      { value: "7d", label: "7d" },
      { value: "month", label: "Month" },
      { value: "quarter", label: "Quarter" },
      { value: "year", label: "Year" },
      { value: "all", label: "All time" },
    ]);
    expect(DEFAULT_PERIOD_KIND).toBe("7d");
  });
});

describe("currentPeriod", () => {
  it("7d is today and the six days before, ending tomorrow 00:00", () => {
    const p = currentPeriod("7d", NOW);
    expect(p.kind).toBe("7d");
    expect(p.id).toBe("7d");
    expect(p.label).toBe("Last 7 days");
    expect(p.start.getTime()).toBe(new Date(2026, 8, 9).getTime());
    expect(p.end.getTime()).toBe(new Date(2026, 8, 16).getTime());
  });

  it("month covers the calendar month", () => {
    const p = currentPeriod("month", NOW);
    expect(p.id).toBe("2026-09");
    expect(p.label).toBe("Sep 2026");
    expect(p.start.getTime()).toBe(new Date(2026, 8, 1).getTime());
    expect(p.end.getTime()).toBe(new Date(2026, 9, 1).getTime());
  });

  it("puts the first and last local instants of a month in the same period", () => {
    expect(currentPeriod("month", new Date(2026, 0, 1, 0, 0, 0)).id).toBe(
      "2026-01",
    );
    expect(
      currentPeriod("month", new Date(2026, 0, 31, 23, 59, 59, 999)).id,
    ).toBe("2026-01");
    expect(currentPeriod("month", new Date(2026, 1, 1)).id).toBe("2026-02");
  });

  it("quarter covers three months", () => {
    const p = currentPeriod("quarter", NOW);
    expect(p.id).toBe("2026-Q3");
    expect(p.label).toBe("Q3 2026");
    expect(ymd(p.start)).toEqual([2026, 7, 1]);
    expect(ymd(p.end)).toEqual([2026, 10, 1]);
    expect(currentPeriod("quarter", new Date(2026, 2, 31)).id).toBe("2026-Q1");
    expect(currentPeriod("quarter", new Date(2026, 3, 1)).id).toBe("2026-Q2");
  });

  it("year covers the calendar year", () => {
    const p = currentPeriod("year", NOW);
    expect(p.id).toBe("2026");
    expect(p.label).toBe("2026");
    expect(ymd(p.start)).toEqual([2026, 1, 1]);
    expect(ymd(p.end)).toEqual([2027, 1, 1]);
  });

  it("knows the length of a leap February", () => {
    const leap = currentPeriod("month", new Date(2028, 1, 10));
    expect(periodDayKeys(leap, new Date(2028, 2, 20))).toHaveLength(29);
    const plain = currentPeriod("month", new Date(2027, 1, 10));
    expect(periodDayKeys(plain, new Date(2027, 2, 20))).toHaveLength(28);
    expect(ymd(leap.end)).toEqual([2028, 3, 1]);
  });
});

describe("previousPeriod", () => {
  it("7d is the 7 days before", () => {
    const prev = priorOf(currentPeriod("7d", NOW));
    expect(prev.kind).toBe("7d");
    expect(ymd(prev.start)).toEqual([2026, 9, 2]);
    expect(ymd(prev.end)).toEqual([2026, 9, 9]);
  });

  it("month rolls Jan back to the previous December", () => {
    const prev = priorOf(currentPeriod("month", new Date(2027, 0, 10)));
    expect(prev.id).toBe("2026-12");
    expect(prev.label).toBe("Dec 2026");
    expect(ymd(prev.end)).toEqual([2027, 1, 1]);
  });

  it("quarter rolls Q1 back to Q4 of the previous year", () => {
    const prev = priorOf(currentPeriod("quarter", new Date(2027, 1, 1)));
    expect(prev.id).toBe("2026-Q4");
    expect(prev.label).toBe("Q4 2026");
    expect(ymd(prev.start)).toEqual([2026, 10, 1]);
    expect(ymd(prev.end)).toEqual([2027, 1, 1]);
  });

  it("year is the year before", () => {
    const prev = priorOf(currentPeriod("year", NOW));
    expect(prev.id).toBe("2025");
    expect(ymd(prev.start)).toEqual([2025, 1, 1]);
  });

  it("walks back over a leap February", () => {
    const mar = currentPeriod("month", new Date(2028, 2, 5));
    const feb = priorOf(mar);
    expect(feb.id).toBe("2028-02");
    expect(ymd(feb.end)).toEqual([2028, 3, 1]);
  });
});

describe("nextPeriod", () => {
  it("is null for the current period of every kind", () => {
    for (const { value } of PERIOD_KINDS) {
      expect(nextPeriod(currentPeriod(value, NOW), NOW)).toBeNull();
    }
  });

  it("steps forward from a past period", () => {
    const aug = priorOf(currentPeriod("month", NOW));
    expect(nextPeriod(aug, NOW)?.id).toBe("2026-09");
    const q2 = priorOf(currentPeriod("quarter", NOW));
    expect(nextPeriod(q2, NOW)?.id).toBe("2026-Q3");
    const y = priorOf(currentPeriod("year", NOW));
    expect(nextPeriod(y, NOW)?.id).toBe("2026");
  });

  it("crosses the year boundary", () => {
    const dec = currentPeriod("month", new Date(2026, 11, 5));
    expect(nextPeriod(dec, new Date(2027, 0, 5))?.id).toBe("2027-01");
    const q4 = currentPeriod("quarter", new Date(2026, 11, 5));
    expect(nextPeriod(q4, new Date(2027, 0, 5))?.id).toBe("2027-Q1");
  });

  it("allows a period that starts today", () => {
    const dec = currentPeriod("month", new Date(2026, 10, 20));
    expect(nextPeriod(dec, new Date(2026, 11, 1, 0, 0, 1))?.id).toBe("2026-12");
    expect(nextPeriod(dec, new Date(2026, 10, 30, 23, 59))).toBeNull();
  });
});

describe("periodFromId", () => {
  it("round trips every kind", () => {
    for (const { value } of PERIOD_KINDS) {
      // All time has no id to pin; it is covered in its own block.
      if (value === "all") continue;
      const p = currentPeriod(value, NOW);
      const back = periodFromId(value, p.id, NOW);
      expect(back?.id).toBe(p.id);
      expect(back?.start.getTime()).toBe(p.start.getTime());
      expect(back?.end.getTime()).toBe(p.end.getTime());
      const prev = priorOf(p);
      expect(periodFromId(value, prev.id, NOW)?.start.getTime()).toBe(
        value === "7d" ? p.start.getTime() : prev.start.getTime(),
      );
    }
  });

  it("rejects ids that are not of the kind", () => {
    expect(periodFromId("7d", "30d", NOW)).toBeUndefined();
    expect(periodFromId("7d", "2026-09", NOW)).toBeUndefined();
    expect(periodFromId("month", "2026-13", NOW)).toBeUndefined();
    expect(periodFromId("month", "2026-00", NOW)).toBeUndefined();
    expect(periodFromId("month", "2026-9", NOW)).toBeUndefined();
    expect(periodFromId("month", "2026-Q3", NOW)).toBeUndefined();
    expect(periodFromId("quarter", "2026-Q5", NOW)).toBeUndefined();
    expect(periodFromId("quarter", "2026-Q0", NOW)).toBeUndefined();
    expect(periodFromId("quarter", "2026-09", NOW)).toBeUndefined();
    expect(periodFromId("year", "26", NOW)).toBeUndefined();
    expect(periodFromId("year", "2026-09", NOW)).toBeUndefined();
    expect(periodFromId("year", "", NOW)).toBeUndefined();
    expect(periodFromId("month", "garbage", NOW)).toBeUndefined();
  });

  it("rejects a period that starts after today", () => {
    expect(periodFromId("month", "2026-10", NOW)).toBeUndefined();
    expect(periodFromId("quarter", "2026-Q4", NOW)).toBeUndefined();
    expect(periodFromId("year", "2027", NOW)).toBeUndefined();
    expect(periodFromId("month", "2026-09", NOW)?.id).toBe("2026-09");
  });
});

describe("pickOf and resolvePick", () => {
  it("follows the current period with a null id and pins a past one", () => {
    for (const { value } of PERIOD_KINDS) {
      expect(pickOf(currentPeriod(value, NOW), NOW)).toEqual({
        kind: value,
        id: null,
      });
    }
    const aug = priorOf(currentPeriod("month", NOW));
    expect(pickOf(aug, NOW)).toEqual({ kind: "month", id: "2026-08" });
  });

  it("resolves a null id to the current period as the days pass", () => {
    const pick = { kind: "month" as const, id: null };
    expect(resolvePick(pick, NOW).id).toBe("2026-09");
    expect(resolvePick(pick, new Date(2026, 9, 2)).id).toBe("2026-10");
  });

  it("resolves a pinned id, or the current period when it is not valid", () => {
    expect(resolvePick({ kind: "quarter", id: "2026-Q1" }, NOW).label).toBe(
      "Q1 2026",
    );
    expect(resolvePick({ kind: "quarter", id: "2027-Q1" }, NOW).id).toBe(
      "2026-Q3",
    );
    expect(resolvePick({ kind: "year", id: "oops" }, NOW).id).toBe("2026");
  });
});

describe("syncWindow", () => {
  it("caps the current period at tomorrow 00:00 and reaches back to the previous start", () => {
    const month = syncWindow(currentPeriod("month", NOW), NOW);
    expect(ymd(month.from)).toEqual([2026, 8, 1]);
    expect(month.to.getTime()).toBe(new Date(2026, 8, 16).getTime());

    const week = syncWindow(currentPeriod("7d", NOW), NOW);
    expect(ymd(week.from)).toEqual([2026, 9, 2]);
    expect(week.to.getTime()).toBe(new Date(2026, 8, 16).getTime());

    const year = syncWindow(currentPeriod("year", NOW), NOW);
    expect(ymd(year.from)).toEqual([2025, 1, 1]);
    expect(year.to.getTime()).toBe(new Date(2026, 8, 16).getTime());
  });

  it("ends a past period at its own end", () => {
    const aug = priorOf(currentPeriod("month", NOW));
    const w = syncWindow(aug, NOW);
    expect(ymd(w.from)).toEqual([2026, 7, 1]);
    expect(ymd(w.to)).toEqual([2026, 9, 1]);
  });
});

describe("previousComparableSpan", () => {
  it("covers the same elapsed days of the previous month", () => {
    const now = new Date(2026, 9, 3, 15); // Oct 3
    const span = previousComparableSpan(currentPeriod("month", now), now);
    expect(ymd(span.start)).toEqual([2026, 9, 1]);
    expect(ymd(span.end)).toEqual([2026, 9, 4]);
  });

  it("is capped at the end of a shorter previous month", () => {
    const now = new Date(2026, 2, 31, 10); // Mar 31 vs 28 days of Feb
    const span = previousComparableSpan(currentPeriod("month", now), now);
    expect(ymd(span.start)).toEqual([2026, 2, 1]);
    expect(ymd(span.end)).toEqual([2026, 3, 1]);
  });

  it("is the whole previous period for a complete past period", () => {
    const quarter = priorOf(currentPeriod("quarter", NOW));
    const span = previousComparableSpan(quarter, NOW);
    const before = priorOf(quarter);
    expect(span.start.getTime()).toBe(before.start.getTime());
    expect(span.end.getTime()).toBe(before.end.getTime());
  });

  it("is the previous 7 days for 7d", () => {
    const span = previousComparableSpan(currentPeriod("7d", NOW), NOW);
    expect(ymd(span.start)).toEqual([2026, 9, 2]);
    expect(ymd(span.end)).toEqual([2026, 9, 9]);
  });

  it("covers the elapsed part of the previous quarter and year", () => {
    const now = new Date(2026, 9, 10, 9); // 10 days into Q4
    const q = previousComparableSpan(currentPeriod("quarter", now), now);
    expect(ymd(q.start)).toEqual([2026, 7, 1]);
    expect(ymd(q.end)).toEqual([2026, 7, 11]);
    const y = previousComparableSpan(currentPeriod("year", NOW), NOW);
    expect(ymd(y.start)).toEqual([2025, 1, 1]);
    expect(ymd(y.end)).toEqual([2025, 9, 16]);
  });

  it("counts calendar days across a DST change", (ctx) => {
    const original = process.env.TZ;
    onTestFinished(() => {
      if (original === undefined) delete process.env.TZ;
      else process.env.TZ = original;
    });
    process.env.TZ = "Europe/Berlin";
    const winter = new Date(2026, 0, 1).getTimezoneOffset();
    const summer = new Date(2026, 6, 1).getTimezoneOffset();
    if (winter === summer) ctx.skip();
    const now = new Date(2026, 3, 2, 10); // Apr 2, after the March change
    const span = previousComparableSpan(currentPeriod("month", now), now);
    expect(ymd(span.start)).toEqual([2026, 3, 1]);
    expect(ymd(span.end)).toEqual([2026, 3, 3]);
    expect(span.end.getHours()).toBe(0);
  });
});

describe("isInPeriod", () => {
  it("includes the start and excludes the end", () => {
    const sep = currentPeriod("month", NOW);
    expect(isInPeriod(sep, new Date(2026, 8, 1, 0, 0).toISOString())).toBe(
      true,
    );
    expect(isInPeriod(sep, new Date(2026, 8, 30, 23, 59).toISOString())).toBe(
      true,
    );
    expect(isInPeriod(sep, new Date(2026, 9, 1, 0, 0).toISOString())).toBe(
      false,
    );
    expect(isInPeriod(sep, new Date(2026, 7, 31, 23, 59).toISOString())).toBe(
      false,
    );
    expect(isInPeriod(sep, "not a date")).toBe(false);
  });

  it("treats a quarter across a year boundary by local time", () => {
    const q4 = currentPeriod("quarter", new Date(2026, 11, 20));
    expect(isInPeriod(q4, new Date(2026, 11, 31, 23, 59).toISOString())).toBe(
      true,
    );
    expect(isInPeriod(q4, new Date(2027, 0, 1, 0, 0).toISOString())).toBe(
      false,
    );
  });
});

describe("localDayKey and isWeekend", () => {
  it("formats the local day and rejects bad timestamps", () => {
    expect(localDayKey(new Date(2026, 9, 5, 23, 30).toISOString())).toBe(
      "2026-10-05",
    );
    expect(localDayKey("nope")).toBe("");
  });

  it("flags Saturday and Sunday only", () => {
    expect(isWeekend("2026-10-10")).toBe(true);
    expect(isWeekend("2026-10-11")).toBe(true);
    expect(isWeekend("2026-10-12")).toBe(false);
    expect(isWeekend("garbage")).toBe(false);
  });
});

describe("periodDayKeys", () => {
  it("lists every day of a past period", () => {
    const aug = priorOf(currentPeriod("month", NOW));
    const keys = periodDayKeys(aug, NOW);
    expect(keys).toHaveLength(31);
    expect(keys[0]).toBe("2026-08-01");
    expect(keys[30]).toBe("2026-08-31");
  });

  it("stops at today for the current period", () => {
    const keys = periodDayKeys(currentPeriod("month", NOW), NOW);
    expect(keys).toHaveLength(15);
    expect(keys[14]).toBe("2026-09-15");
    const year = periodDayKeys(currentPeriod("year", NOW), NOW);
    expect(year).toHaveLength(31 + 28 + 31 + 30 + 31 + 30 + 31 + 31 + 15);
  });

  it("has seven days for the 7d period", () => {
    expect(periodDayKeys(currentPeriod("7d", NOW), NOW)).toEqual([
      "2026-09-09",
      "2026-09-10",
      "2026-09-11",
      "2026-09-12",
      "2026-09-13",
      "2026-09-14",
      "2026-09-15",
    ]);
  });
});

describe("daylight saving time", () => {
  const original = process.env.TZ;
  afterEach(() => {
    if (original === undefined) delete process.env.TZ;
    else process.env.TZ = original;
  });

  it("counts 7 day keys in the week of the March change in Europe/Berlin", (ctx) => {
    process.env.TZ = "Europe/Berlin";
    // Setting TZ at runtime is not honored everywhere; skip rather than lie.
    const winter = new Date(2026, 0, 1).getTimezoneOffset();
    const summer = new Date(2026, 6, 1).getTimezoneOffset();
    if (winter === summer) ctx.skip();
    // DST starts Sunday 2026-03-29: that day has 23 hours.
    const now = new Date(2026, 2, 30, 10);
    const p = currentPeriod("7d", now);
    expect(p.start.getHours()).toBe(0);
    expect(p.end.getHours()).toBe(0);
    expect(periodDayKeys(p, now)).toEqual([
      "2026-03-24",
      "2026-03-25",
      "2026-03-26",
      "2026-03-27",
      "2026-03-28",
      "2026-03-29",
      "2026-03-30",
    ]);
    // The week that ends on the change day still has 7 keys.
    const sunday = new Date(2026, 2, 29, 12);
    expect(periodDayKeys(currentPeriod("7d", sunday), sunday)).toHaveLength(7);
    expect(periodDayKeys(priorOf(p), now)).toHaveLength(7);
  });

  it("keeps month boundaries at local midnight across a DST change", (ctx) => {
    process.env.TZ = "America/New_York";
    const winter = new Date(2026, 0, 1).getTimezoneOffset();
    const summer = new Date(2026, 6, 1).getTimezoneOffset();
    if (winter === summer) ctx.skip();
    // DST ends 2026-11-01 in New York: November starts on a 25 hour day.
    const nov = currentPeriod("month", new Date(2026, 10, 15));
    expect(nov.start.getHours()).toBe(0);
    expect(nov.end.getHours()).toBe(0);
    expect(periodDayKeys(nov, new Date(2026, 11, 20))).toHaveLength(30);
    expect(priorOf(nov).id).toBe("2026-10");
  });
});

describe("all time", () => {
  const FIRST = new Date(2023, 4, 17, 15, 40); // Wed 2023-05-17 15:40

  it("starts on the local day of the first commit and ends tomorrow 00:00", () => {
    const p = currentPeriod("all", NOW, FIRST);
    expect(p.kind).toBe("all");
    expect(p.id).toBe("all");
    expect(p.label).toBe("All time");
    expect(p.start.getTime()).toBe(new Date(2023, 4, 17).getTime());
    expect(p.end.getTime()).toBe(new Date(2026, 8, 16).getTime());
  });

  it("covers today only until the first commit is known, or when there is none", () => {
    for (const unknown of [undefined, null]) {
      const p = currentPeriod("all", NOW, unknown);
      expect(p.start.getTime()).toBe(new Date(2026, 8, 15).getTime());
      expect(p.end.getTime()).toBe(new Date(2026, 8, 16).getTime());
    }
  });

  it("never starts after today, even for a first commit dated in the future", () => {
    const p = currentPeriod("all", NOW, new Date(2027, 0, 5));
    expect(p.start.getTime()).toBe(new Date(2026, 8, 15).getTime());
  });

  it("has no previous period and no next one", () => {
    const p = currentPeriod("all", NOW, FIRST);
    expect(previousPeriod(p)).toBeNull();
    expect(nextPeriod(p, NOW)).toBeNull();
  });

  it("syncs exactly [start, tomorrow), without a doubled window", () => {
    const p = currentPeriod("all", NOW, FIRST);
    const w = syncWindow(p, NOW);
    expect(w.from.getTime()).toBe(new Date(2023, 4, 17).getTime());
    expect(w.to.getTime()).toBe(new Date(2026, 8, 16).getTime());
  });

  it("has nothing to compare with", () => {
    expect(
      previousComparableSpan(currentPeriod("all", NOW, FIRST), NOW),
    ).toBeNull();
  });

  it("has no id to pin: the id never resolves to a period", () => {
    expect(periodFromId("all", "all", NOW)).toBeUndefined();
    expect(pickOf(currentPeriod("all", NOW, FIRST), NOW)).toEqual({
      kind: "all",
      id: null,
    });
  });

  it("resolves a pick of all through the first commit", () => {
    const p = resolvePick({ kind: "all", id: null }, NOW, FIRST);
    expect(p.start.getTime()).toBe(new Date(2023, 4, 17).getTime());
    expect(p.label).toBe("All time");
  });

  it("counts its day keys from the first day to today", () => {
    const first = new Date(2026, 8, 12, 9);
    expect(periodDayKeys(currentPeriod("all", NOW, first), NOW)).toEqual([
      "2026-09-12",
      "2026-09-13",
      "2026-09-14",
      "2026-09-15",
    ]);
  });

  it("contains commits from the first day on and none after tomorrow", () => {
    const p = currentPeriod("all", NOW, FIRST);
    expect(isInPeriod(p, new Date(2023, 4, 17, 0, 1).toISOString())).toBe(true);
    expect(isInPeriod(p, new Date(2023, 4, 16, 23, 59).toISOString())).toBe(
      false,
    );
    expect(isInPeriod(p, NOW.toISOString())).toBe(true);
  });
});
