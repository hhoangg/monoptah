import { previousSeason, seasonOf } from "./season";
import type { Season } from "./season";

export type RangeKey = "7d" | "30d" | "90d" | "1y" | "all";

export const RANGE_OPTIONS: { value: RangeKey; label: string }[] = [
  { value: "7d", label: "7d" },
  { value: "30d", label: "30d" },
  { value: "90d", label: "90d" },
  { value: "1y", label: "1y" },
  { value: "all", label: "All" },
];

const RANGE_DAYS: Record<Exclude<RangeKey, "all">, number> = {
  "7d": 7,
  "30d": 30,
  "90d": 90,
  "1y": 365,
};

/** Older than any git history, so `all` never needs a repository-specific date. */
export const ALL_SINCE = "1990-01-01T00:00:00Z";

export const DEFAULT_RANGE: RangeKey = "30d";

export function parseRange(
  value: string | null | undefined,
): RangeKey | undefined {
  return value === "7d" ||
    value === "30d" ||
    value === "90d" ||
    value === "1y" ||
    value === "all"
    ? value
    : undefined;
}

const startOfDay = (date: Date) =>
  new Date(date.getFullYear(), date.getMonth(), date.getDate());

const isoSeconds = (date: Date) => date.toISOString().replace(/\.\d{3}Z$/, "Z");

/**
 * The shown period as whole local days: N days ending with today. `all`
 * starts at the earliest commit (`earliest`), or 30 days back without one.
 */
export function rangeBounds(
  range: RangeKey,
  now: Date,
  earliest?: Date,
): { start: Date; end: Date } {
  const today = startOfDay(now);
  const end = new Date(
    today.getFullYear(),
    today.getMonth(),
    today.getDate() + 1,
  );
  if (range === "all") {
    const start = earliest ? startOfDay(earliest) : null;
    return {
      start:
        start ??
        new Date(end.getFullYear(), end.getMonth(), end.getDate() - 30),
      end,
    };
  }
  const days = RANGE_DAYS[range];
  return {
    start: new Date(end.getFullYear(), end.getMonth(), end.getDate() - days),
    end,
  };
}

/**
 * The `since` to collect with, day granular so the cache key is stable through
 * the day. It reaches back to the earlier of the previous season start (rank
 * movement) and, for 7d, 30d and 90d, the start of the equal-length period
 * before the range, so the KPI deltas always have data to compare with.
 * 1y does not: that would fetch two years, so its deltas stay hidden (the
 * stats report null when the data does not cover the previous period). `all`
 * has nothing earlier to compare with.
 * A selected `season` replaces the current one in that rule, so its rank
 * movement has data; when the range already reaches that far the result (and
 * the cache key built from it) is the same as without a season.
 */
export function sinceFor(
  range: RangeKey,
  now: Date,
  season: Season = seasonOf(now),
): string {
  if (range === "all") return ALL_SINCE;
  const { start, end } = rangeBounds(range, now);
  const seasonStart = previousSeason(season).start;
  const previousStart =
    range === "1y"
      ? start
      : new Date(start.getTime() - (end.getTime() - start.getTime()));
  return isoSeconds(previousStart < seasonStart ? previousStart : seasonStart);
}

const SEASON_CHOICES = 6;

/** The seasons the leaderboard can show: the current month and the five before it, newest first. */
export function seasonChoices(now: Date): Season[] {
  const choices = [seasonOf(now)];
  while (choices.length < SEASON_CHOICES) {
    choices.push(previousSeason(choices[choices.length - 1]));
  }
  return choices;
}
