export type PeriodKind = "7d" | "month" | "quarter" | "year" | "all";

/** A span of whole local days. `end` is exclusive and is a local midnight. */
export type Period = {
  kind: PeriodKind;
  /** `7d`, `2026-09`, `2026-Q3`, `2026` or `all`. */
  id: string;
  /** `Last 7 days`, `Sep 2026`, `Q3 2026`, `2026` or `All time`. */
  label: string;
  start: Date;
  end: Date;
};

export const PERIOD_KINDS: { value: PeriodKind; label: string }[] = [
  { value: "7d", label: "7d" },
  { value: "month", label: "Month" },
  { value: "quarter", label: "Quarter" },
  { value: "year", label: "Year" },
  { value: "all", label: "All time" },
];

export const DEFAULT_PERIOD_KIND: PeriodKind = "7d";

const MONTHS = [
  "Jan",
  "Feb",
  "Mar",
  "Apr",
  "May",
  "Jun",
  "Jul",
  "Aug",
  "Sep",
  "Oct",
  "Nov",
  "Dec",
];

const pad2 = (n: number) => String(n).padStart(2, "0");

const startOfDay = (date: Date) =>
  new Date(date.getFullYear(), date.getMonth(), date.getDate());

const addDays = (date: Date, days: number) =>
  new Date(date.getFullYear(), date.getMonth(), date.getDate() + days);

/** `YYYY-MM-DD` of a local calendar day. */
const dayKeyOf = (date: Date) =>
  `${date.getFullYear()}-${pad2(date.getMonth() + 1)}-${pad2(date.getDate())}`;

/** The last 7 days ending the day before `endExclusive`. */
function lastSevenDays(endExclusive: Date): Period {
  return {
    kind: "7d",
    id: "7d",
    label: "Last 7 days",
    start: addDays(endExclusive, -7),
    end: endExclusive,
  };
}

// Month, quarter and year are built from a month index; overflow such as month
// -1 or 12 is normalized through Date.
function monthPeriod(year: number, month: number): Period {
  const start = new Date(year, month, 1);
  const y = start.getFullYear();
  const m = start.getMonth();
  return {
    kind: "month",
    id: `${y}-${pad2(m + 1)}`,
    label: `${MONTHS[m]} ${y}`,
    start,
    end: new Date(y, m + 1, 1),
  };
}

function quarterPeriod(year: number, quarter: number): Period {
  const start = new Date(year, quarter * 3, 1);
  const y = start.getFullYear();
  const q = start.getMonth() / 3;
  return {
    kind: "quarter",
    id: `${y}-Q${q + 1}`,
    label: `Q${q + 1} ${y}`,
    start,
    end: new Date(y, start.getMonth() + 3, 1),
  };
}

function yearPeriod(year: number): Period {
  return {
    kind: "year",
    id: String(year),
    label: String(year),
    start: new Date(year, 0, 1),
    end: new Date(year + 1, 0, 1),
  };
}

/**
 * Everything from the day of the first commit to today. It has no period
 * before it, so there is nothing to compare with.
 */
function allTimePeriod(
  firstCommit: Date | null | undefined,
  now: Date,
): Period {
  const today = startOfDay(now);
  const first = firstCommit ? startOfDay(firstCommit) : today;
  return {
    kind: "all",
    id: "all",
    label: "All time",
    start: first.getTime() < today.getTime() ? first : today,
    end: addDays(now, 1),
  };
}

/**
 * The period of `kind` that contains `now` (for 7d, the 7 days ending today).
 * All time starts on the day of `firstCommit`; without one (not known yet, or
 * a repository without commits) it covers today only.
 */
export function currentPeriod(
  kind: PeriodKind,
  now: Date,
  firstCommit?: Date | null,
): Period {
  switch (kind) {
    case "7d":
      return lastSevenDays(addDays(now, 1));
    case "month":
      return monthPeriod(now.getFullYear(), now.getMonth());
    case "quarter":
      return quarterPeriod(now.getFullYear(), Math.floor(now.getMonth() / 3));
    case "year":
      return yearPeriod(now.getFullYear());
    case "all":
      return allTimePeriod(firstCommit, now);
  }
}

/** The period of the same kind right before `p`; null for All time, which has none. */
export function previousPeriod(p: Period): Period | null {
  switch (p.kind) {
    case "7d":
      return lastSevenDays(p.start);
    case "month":
      return monthPeriod(p.start.getFullYear(), p.start.getMonth() - 1);
    case "quarter":
      return quarterPeriod(p.start.getFullYear(), p.start.getMonth() / 3 - 1);
    case "year":
      return yearPeriod(p.start.getFullYear() - 1);
    case "all":
      return null;
  }
}

/** The period after `p`, or null when it would start after today (and always for 7d and All time). */
export function nextPeriod(p: Period, now: Date): Period | null {
  let next: Period;
  switch (p.kind) {
    case "7d":
    case "all":
      return null;
    case "month":
      next = monthPeriod(p.start.getFullYear(), p.start.getMonth() + 1);
      break;
    case "quarter":
      next = quarterPeriod(p.start.getFullYear(), p.start.getMonth() / 3 + 1);
      break;
    case "year":
      next = yearPeriod(p.start.getFullYear() + 1);
      break;
  }
  return next.start.getTime() > startOfDay(now).getTime() ? null : next;
}

/**
 * The period a saved id stands for, or undefined when the id is not one of
 * `kind` or names a period that starts after today. The last 7 days and All
 * time have nothing to pin, so no id stands for either.
 */
export function periodFromId(
  kind: PeriodKind,
  id: string,
  now: Date,
): Period | undefined {
  let found: Period | undefined;
  if (kind === "all") {
    return undefined;
  } else if (kind === "7d") {
    found = id === "7d" ? currentPeriod("7d", now) : undefined;
  } else if (kind === "month") {
    const m = /^(\d{4})-(\d{2})$/.exec(id);
    const month = m ? Number(m[2]) : 0;
    if (m && month >= 1 && month <= 12) {
      found = monthPeriod(Number(m[1]), month - 1);
    }
  } else if (kind === "quarter") {
    const m = /^(\d{4})-Q([1-4])$/.exec(id);
    if (m) found = quarterPeriod(Number(m[1]), Number(m[2]) - 1);
  } else {
    const m = /^(\d{4})$/.exec(id);
    if (m) found = yearPeriod(Number(m[1]));
  }
  if (!found || found.id !== id) return undefined;
  if (found.start.getTime() > startOfDay(now).getTime()) return undefined;
  return found;
}

/**
 * The days a period needs commits for: from the start of the previous period
 * (rank movement and KPI deltas compare with it) to the end of `p`, capped at
 * tomorrow 00:00 so the current period does not reach into the future. All
 * time has no previous period, so it needs only its own days.
 */
export function syncWindow(p: Period, now: Date): { from: Date; to: Date } {
  const tomorrow = addDays(now, 1);
  return {
    from: (previousPeriod(p) ?? p).start,
    to: p.end.getTime() < tomorrow.getTime() ? p.end : tomorrow,
  };
}

/**
 * The span of the previous period that matches how much of `p` has elapsed, so
 * a partly elapsed period is compared with the same days of the one before it:
 * the 3rd of October against Sep 1-3, not the whole of September. A complete
 * period compares whole with whole. Built from calendar days, so a DST change
 * does not shift the end. Null when there is no previous period (All time).
 */
export function previousComparableSpan(
  p: Period,
  now: Date,
): { start: Date; end: Date } | null {
  const before = previousPeriod(p);
  if (!before) return null;
  const elapsedEnd = syncWindow(p, now).to;
  const utcDay = (d: Date) =>
    Date.UTC(d.getFullYear(), d.getMonth(), d.getDate());
  const days = Math.round((utcDay(elapsedEnd) - utcDay(p.start)) / 86_400_000);
  const end = addDays(before.start, days);
  return {
    start: before.start,
    end: end.getTime() < before.end.getTime() ? end : before.end,
  };
}

export function isInPeriod(p: Period, iso: string): boolean {
  const t = new Date(iso).getTime();
  if (Number.isNaN(t)) return false;
  return t >= p.start.getTime() && t < p.end.getTime();
}

/** "YYYY-MM-DD" of the local calendar day, or "" for an unparseable timestamp. */
export function localDayKey(iso: string): string {
  const d = new Date(iso);
  if (Number.isNaN(d.getTime())) return "";
  return dayKeyOf(d);
}

export function isWeekend(dayKey: string): boolean {
  const m = /^(\d{4})-(\d{2})-(\d{2})$/.exec(dayKey);
  if (!m) return false;
  const day = new Date(Number(m[1]), Number(m[2]) - 1, Number(m[3])).getDay();
  return day === 0 || day === 6;
}

/** Local day keys of the period, oldest first, not beyond today. */
export function periodDayKeys(p: Period, now: Date): string[] {
  const today = startOfDay(now).getTime();
  const keys: string[] = [];
  for (let d = p.start; d.getTime() < p.end.getTime(); d = addDays(d, 1)) {
    if (d.getTime() > today) break;
    keys.push(dayKeyOf(d));
  }
  return keys;
}

/**
 * What the user selected. A null `id` follows the current period of the kind,
 * so a window left open across midnight (or a month change) moves with the
 * calendar; an id pins a past period.
 */
export type PeriodPick = { kind: PeriodKind; id: string | null };

/** The pick that selects `p`: pinned, unless `p` is the current period of its kind. */
export function pickOf(p: Period, now: Date): PeriodPick {
  return {
    kind: p.kind,
    // All time is always the current one: its id is a constant, not a place on the calendar.
    id:
      p.kind === "all" || p.id === currentPeriod(p.kind, now).id ? null : p.id,
  };
}

/**
 * The period a pick stands for today; a pinned id that is no longer valid
 * reads as the current one. `firstCommit` is where All time starts.
 */
export function resolvePick(
  pick: PeriodPick,
  now: Date,
  firstCommit?: Date | null,
): Period {
  return (
    (pick.id !== null ? periodFromId(pick.kind, pick.id, now) : undefined) ??
    currentPeriod(pick.kind, now, firstCommit)
  );
}
