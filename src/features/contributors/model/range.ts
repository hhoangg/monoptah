import { previousSeason, seasonOf } from "./season";
import type { Season } from "./season";

/** The fixed ranges, N whole local days ending with today. */
export type PresetKey = "7d" | "30d";

/** What the range control selects between. */
export type RangeKey = PresetKey | "custom";

/** A range of whole local days, both ends inclusive, as `YYYY-MM-DD`. */
export type CustomRange = { key: "custom"; start: string; end: string };

/** The chosen range: a preset, or explicit days. */
export type RangeValue = PresetKey | CustomRange;

export const RANGE_OPTIONS: { value: RangeKey; label: string }[] = [
  { value: "7d", label: "7d" },
  { value: "30d", label: "30d" },
  { value: "custom", label: "Custom" },
];

const PRESET_DAYS: Record<PresetKey, number> = { "7d": 7, "30d": 30 };

/** The longest span a custom range may cover, both end days counted. */
export const MAX_CUSTOM_DAYS = 30;

export const DEFAULT_RANGE: PresetKey = "30d";

const DAY_MS = 24 * 3600 * 1000;

const pad2 = (n: number) => String(n).padStart(2, "0");

const startOfDay = (date: Date) =>
  new Date(date.getFullYear(), date.getMonth(), date.getDate());

const addDays = (date: Date, days: number) =>
  new Date(date.getFullYear(), date.getMonth(), date.getDate() + days);

const isoSeconds = (date: Date) => date.toISOString().replace(/\.\d{3}Z$/, "Z");

/** `YYYY-MM-DD` of a local calendar day. */
export function formatDay(date: Date): string {
  return `${date.getFullYear()}-${pad2(date.getMonth() + 1)}-${pad2(date.getDate())}`;
}

/** The local day of a `YYYY-MM-DD` text, or undefined for anything else (including 2026-02-31). */
export function parseDay(text: string): Date | undefined {
  const m = /^(\d{4})-(\d{2})-(\d{2})$/.exec(text);
  if (!m) return undefined;
  const date = new Date(Number(m[1]), Number(m[2]) - 1, Number(m[3]));
  return formatDay(date) === text ? date : undefined;
}

/**
 * The earliest day a custom range may reach: the start of the season before the
 * oldest selectable one, which is as far back as season selection already
 * collects. A far-past range would otherwise collect years of history.
 */
export function earliestCustomDay(now: Date): Date {
  const choices = seasonChoices(now);
  return previousSeason(choices[choices.length - 1]).start;
}

/**
 * A custom range from two days, forced to be valid: it neither starts nor ends
 * before `earliestCustomDay`, ends no later than today, starts no later than
 * it ends, and spans at most `MAX_CUSTOM_DAYS`. When the span is too long the
 * end is kept and the start moves.
 */
export function customRange(start: Date, end: Date, now: Date): CustomRange {
  const today = startOfDay(now);
  const bound = earliestCustomDay(now);
  let last = startOfDay(end) < today ? startOfDay(end) : today;
  if (last < bound) last = bound;
  let first = startOfDay(start);
  if (first > last) first = last;
  const earliest = addDays(last, -(MAX_CUSTOM_DAYS - 1));
  if (first < earliest) first = earliest;
  if (first < bound) first = bound;
  return { key: "custom", start: formatDay(first), end: formatDay(last) };
}

/**
 * The range after the user picks a new first day. The other end follows when
 * the pick would make the span invalid, so any day within the allowed window
 * (`earliestCustomDay` to today) can be picked.
 */
export function withStart(
  range: CustomRange,
  pick: Date,
  now: Date,
): CustomRange {
  const end = parseDay(range.end) ?? startOfDay(now);
  const bound = earliestCustomDay(now);
  const picked = startOfDay(pick);
  const first = picked < bound ? bound : picked;
  const last =
    first > end
      ? first
      : first < addDays(end, -(MAX_CUSTOM_DAYS - 1))
        ? addDays(first, MAX_CUSTOM_DAYS - 1)
        : end;
  return customRange(first, last, now);
}

/** Like `withStart`, for a new last day. */
export function withEnd(
  range: CustomRange,
  pick: Date,
  now: Date,
): CustomRange {
  const start = parseDay(range.start) ?? startOfDay(now);
  const bound = earliestCustomDay(now);
  const picked = startOfDay(pick);
  const last = picked < bound ? bound : picked;
  const first =
    last < start
      ? last
      : last > addDays(start, MAX_CUSTOM_DAYS - 1)
        ? addDays(last, -(MAX_CUSTOM_DAYS - 1))
        : start;
  return customRange(first, last, now);
}

/** The selector key of a range. */
export const rangeKeyOf = (range: RangeValue): RangeKey =>
  typeof range === "string" ? range : "custom";

/** What a saved preference holds: `7d`, `30d` or `custom:YYYY-MM-DD:YYYY-MM-DD`. */
export function serializeRange(range: RangeValue): string {
  return typeof range === "string"
    ? range
    : `custom:${range.start}:${range.end}`;
}

/**
 * The range a saved text stands for, or undefined when it is not one (this
 * includes the removed `90d`, `1y` and `all`). A custom range that is too long
 * or ends after today is clamped, not dropped.
 */
export function parseRange(
  value: string | null | undefined,
  now: Date = new Date(),
): RangeValue | undefined {
  if (value === "7d" || value === "30d") return value;
  const m = /^custom:(\d{4}-\d{2}-\d{2}):(\d{4}-\d{2}-\d{2})$/.exec(
    value ?? "",
  );
  if (!m) return undefined;
  const start = parseDay(m[1]);
  const end = parseDay(m[2]);
  return start && end ? customRange(start, end, now) : undefined;
}

/**
 * The shown period as whole local days. A preset is N days ending with today;
 * a custom range is its two days, clamped like `customRange`. `end` is the
 * start of the day after the last one.
 */
export function rangeBounds(
  range: RangeValue,
  now: Date,
): { start: Date; end: Date } {
  if (typeof range === "string") {
    const tomorrow = addDays(now, 1);
    return { start: addDays(tomorrow, -PRESET_DAYS[range]), end: tomorrow };
  }
  const start = parseDay(range.start) ?? startOfDay(now);
  const end = parseDay(range.end) ?? startOfDay(now);
  const clamped = customRange(start, end, now);
  return {
    start: parseDay(clamped.start)!,
    end: addDays(parseDay(clamped.end)!, 1),
  };
}

/** The range as explicit days, so choosing Custom starts from the period already shown. */
export function toCustom(range: RangeValue, now: Date): CustomRange {
  if (typeof range !== "string")
    return customRange(
      parseDay(range.start) ?? now,
      parseDay(range.end) ?? now,
      now,
    );
  const { start, end } = rangeBounds(range, now);
  return customRange(start, addDays(end, -1), now);
}

/** Whole days in the shown period. */
export function rangeDays(range: RangeValue, now: Date): number {
  const { start, end } = rangeBounds(range, now);
  return Math.round((end.getTime() - start.getTime()) / DAY_MS);
}

/** Short text for a range: `7d`, `30d`, or `2026-10-01 to 2026-10-15`. */
export function rangeLabel(range: RangeValue, now: Date): string {
  if (typeof range === "string") return range;
  const { start, end } = rangeBounds(range, now);
  return `${formatDay(start)} to ${formatDay(addDays(end, -1))}`;
}

/**
 * The `since` to collect with, day granular so the cache key is stable through
 * the day. It reaches back to the earlier of the previous season start (rank
 * movement) and the start of the equal-length period before the range, so the
 * KPI deltas always have data to compare with. The end stays open (now): the
 * leaderboard ranks a season independently of the range, so a custom range in
 * the past must not cut off the data of the current month.
 * A selected `season` replaces the current one in that rule, so its rank
 * movement has data; when the range already reaches that far the result (and
 * the cache key built from it) is the same as without a season.
 */
export function sinceFor(
  range: RangeValue,
  now: Date,
  season: Season = seasonOf(now),
): string {
  const { start, end } = rangeBounds(range, now);
  const seasonStart = previousSeason(season).start;
  const previousStart = new Date(
    start.getTime() - (end.getTime() - start.getTime()),
  );
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
