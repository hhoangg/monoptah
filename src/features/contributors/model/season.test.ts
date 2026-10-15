import { afterEach, describe, expect, it } from "vitest";
import {
  isInSeason,
  isWeekend,
  localDayKey,
  previousSeason,
  seasonById,
  seasonDayKeys,
  seasonOf,
} from "./season";

describe("seasonOf", () => {
  it("uses the local calendar month", () => {
    const s = seasonOf(new Date(2026, 9, 15, 12, 0, 0));
    expect(s.id).toBe("2026-10");
    expect(s.label).toBe("Oct 2026");
    expect(s.start.getTime()).toBe(new Date(2026, 9, 1).getTime());
    expect(s.end.getTime()).toBe(new Date(2026, 10, 1).getTime());
  });

  it("puts the first and last local instants in the same month", () => {
    expect(seasonOf(new Date(2026, 0, 1, 0, 0, 0)).id).toBe("2026-01");
    expect(seasonOf(new Date(2026, 0, 31, 23, 59, 59, 999)).id).toBe("2026-01");
    expect(seasonOf(new Date(2026, 1, 1, 0, 0, 0)).id).toBe("2026-02");
  });

  it("zero-pads the month and rolls the year for December", () => {
    const s = seasonOf(new Date(2026, 11, 20));
    expect(s.id).toBe("2026-12");
    expect(s.label).toBe("Dec 2026");
    expect(s.end.getTime()).toBe(new Date(2027, 0, 1).getTime());
  });

  it("labels every month in English", () => {
    const labels = Array.from(
      { length: 12 },
      (_, m) => seasonOf(new Date(2026, m, 10)).label,
    );
    expect(labels).toEqual([
      "Jan 2026",
      "Feb 2026",
      "Mar 2026",
      "Apr 2026",
      "May 2026",
      "Jun 2026",
      "Jul 2026",
      "Aug 2026",
      "Sep 2026",
      "Oct 2026",
      "Nov 2026",
      "Dec 2026",
    ]);
  });
});

describe("previousSeason", () => {
  it("steps back one month", () => {
    expect(previousSeason(seasonOf(new Date(2026, 9, 5))).id).toBe("2026-09");
  });

  it("crosses the year boundary", () => {
    const prev = previousSeason(seasonOf(new Date(2026, 0, 5)));
    expect(prev.id).toBe("2025-12");
    expect(prev.label).toBe("Dec 2025");
    expect(prev.end.getTime()).toBe(new Date(2026, 0, 1).getTime());
  });
});

describe("seasonById", () => {
  it("parses a valid id", () => {
    const s = seasonById("2026-10");
    expect(s?.label).toBe("Oct 2026");
    expect(s?.start.getTime()).toBe(new Date(2026, 9, 1).getTime());
  });

  it("returns null for malformed ids", () => {
    expect(seasonById("")).toBeNull();
    expect(seasonById("2026-13")).toBeNull();
    expect(seasonById("2026-00")).toBeNull();
    expect(seasonById("2026-1")).toBeNull();
    expect(seasonById("abc")).toBeNull();
    expect(seasonById("2026-10-01")).toBeNull();
  });

  it("round-trips with seasonOf", () => {
    const s = seasonOf(new Date(2025, 2, 9));
    expect(seasonById(s.id)?.start.getTime()).toBe(s.start.getTime());
  });
});

describe("isInSeason", () => {
  const oct = seasonOf(new Date(2026, 9, 1));

  it("includes start and excludes end", () => {
    expect(isInSeason(oct, new Date(2026, 9, 1, 0, 0, 0).toISOString())).toBe(
      true,
    );
    expect(
      isInSeason(oct, new Date(2026, 9, 31, 23, 59, 59).toISOString()),
    ).toBe(true);
    expect(isInSeason(oct, new Date(2026, 10, 1, 0, 0, 0).toISOString())).toBe(
      false,
    );
    expect(
      isInSeason(oct, new Date(2026, 8, 30, 23, 59, 59).toISOString()),
    ).toBe(false);
  });

  it("is false for unparseable input", () => {
    expect(isInSeason(oct, "not a date")).toBe(false);
    expect(isInSeason(oct, "")).toBe(false);
  });
});

describe("localDayKey", () => {
  it("formats the local day", () => {
    expect(localDayKey(new Date(2026, 9, 5, 23, 30).toISOString())).toBe(
      "2026-10-05",
    );
    expect(localDayKey(new Date(2026, 0, 1, 0, 5).toISOString())).toBe(
      "2026-01-01",
    );
  });

  it("returns an empty string for invalid input", () => {
    expect(localDayKey("nope")).toBe("");
  });
});

describe("isWeekend", () => {
  it("detects Saturday and Sunday", () => {
    expect(isWeekend("2026-10-17")).toBe(true); // Saturday
    expect(isWeekend("2026-10-18")).toBe(true); // Sunday
    expect(isWeekend("2026-10-16")).toBe(false); // Friday
    expect(isWeekend("2026-10-19")).toBe(false); // Monday
  });

  it("is false for malformed keys", () => {
    expect(isWeekend("")).toBe(false);
    expect(isWeekend("garbage")).toBe(false);
  });
});

describe("seasonDayKeys", () => {
  it("lists every day of the month in order", () => {
    const keys = seasonDayKeys(seasonOf(new Date(2026, 9, 1)));
    expect(keys).toHaveLength(31);
    expect(keys[0]).toBe("2026-10-01");
    expect(keys[30]).toBe("2026-10-31");
  });

  it("handles short months and leap years", () => {
    expect(seasonDayKeys(seasonOf(new Date(2026, 1, 1)))).toHaveLength(28);
    expect(seasonDayKeys(seasonOf(new Date(2028, 1, 1)))).toHaveLength(29);
    expect(seasonDayKeys(seasonOf(new Date(2026, 3, 1)))).toHaveLength(30);
  });
});

describe("year change", () => {
  it("rolls from December to January", () => {
    const dec = seasonOf(new Date(2026, 11, 31, 23, 59, 59, 999));
    const jan = seasonOf(new Date(2027, 0, 1, 0, 0, 0, 0));
    expect(dec.id).toBe("2026-12");
    expect(jan.id).toBe("2027-01");
    expect(dec.end.getTime()).toBe(jan.start.getTime());
    expect(previousSeason(jan).id).toBe(dec.id);
  });

  it("buckets days around the new year", () => {
    expect(localDayKey(new Date(2026, 11, 31, 23, 59).toISOString())).toBe(
      "2026-12-31",
    );
    expect(localDayKey(new Date(2027, 0, 1, 0, 1).toISOString())).toBe(
      "2027-01-01",
    );
    const jan = seasonOf(new Date(2027, 0, 15));
    expect(isInSeason(jan, new Date(2026, 11, 31, 23, 59).toISOString())).toBe(
      false,
    );
    expect(isInSeason(jan, new Date(2027, 0, 1, 0, 0).toISOString())).toBe(
      true,
    );
  });
});

describe("daylight saving time", () => {
  const original = process.env.TZ;
  afterEach(() => {
    if (original === undefined) delete process.env.TZ;
    else process.env.TZ = original;
  });

  it("keeps season boundaries at local midnight across a DST change", (ctx) => {
    process.env.TZ = "America/New_York";
    // Setting TZ at runtime is not honored everywhere; skip rather than lie.
    const winter = new Date(2026, 0, 1).getTimezoneOffset();
    const summer = new Date(2026, 6, 1).getTimezoneOffset();
    if (winter === summer) ctx.skip();
    // DST ends 2026-11-01 in New York: November has 25 hours on day one.
    const nov = seasonOf(new Date(2026, 10, 15));
    expect(nov.start.getHours()).toBe(0);
    expect(nov.end.getHours()).toBe(0);
    expect(nov.end.getMonth()).toBe(11);
    expect(seasonDayKeys(nov)).toHaveLength(30);
    expect(seasonOf(new Date(2026, 9, 31, 23, 30)).id).toBe("2026-10");
    expect(previousSeason(nov).id).toBe("2026-10");
  });
});
