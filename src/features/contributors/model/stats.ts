import { identityKey, isHuman } from "./identity";
import type { Resolver } from "./identity";
import { colorFromMap } from "./palette";
import { attributeCommits, attributePrs } from "./scoring";
import type { AttributedCommit } from "./scoring";
import { localDayKey } from "./period";
import type { Period } from "./period";
import type { ContributorsConfig, RawData } from "./types";

export type StatsRow = {
  personId: string;
  name: string;
  commits: number;
  added: number;
  deleted: number;
  files: number;
  aiAssistedPct: number;
  sharePct: number;
  lastActive: string | null;
  identities: string[];
  color: string;
};

export type BucketGranularity = "day" | "week" | "month" | "year";

/** The most months All time keeps on a monthly axis; longer spans switch to years. */
const MAX_MONTH_BUCKETS = 48;

/**
 * Chart buckets for a period: days for 7d and month, weeks for a quarter,
 * months for a year. All time uses months while they fit on the axis
 * (MAX_MONTH_BUCKETS) and years beyond that.
 */
export function bucketGranularity(period: Period): BucketGranularity {
  switch (period.kind) {
    case "7d":
    case "month":
      return "day";
    case "quarter":
      return "week";
    case "year":
      return "month";
    case "all":
      return calendarMonthsTouched(period) <= MAX_MONTH_BUCKETS
        ? "month"
        : "year";
  }
}

/** How many calendar months the period touches, first and last included. */
function calendarMonthsTouched(period: Period): number {
  // `end` is exclusive: step back to the last day the period covers.
  const last = new Date(
    period.end.getFullYear(),
    period.end.getMonth(),
    period.end.getDate() - 1,
  );
  return (
    (last.getFullYear() - period.start.getFullYear()) * 12 +
    (last.getMonth() - period.start.getMonth()) +
    1
  );
}

export type Bucket = {
  /** Local day key of the first day of the day, week (Monday), month or year. */
  start: string;
  /** Tooltip text: "Oct 5, 2026", "Week of Oct 5, 2026", "Oct 2026" or "2026". */
  label: string;
  /** Axis text: "Oct 5", "Oct 2026" or "2026". */
  short: string;
  byPerson: Record<string, number>;
};

export type Stats = {
  kpis: {
    commits: { value: number; delta: number | null };
    contributors: { value: number; identities: number; newcomers: number };
    /** `delta` is the fraction change in lines changed (added + deleted). */
    lines: { added: number; deleted: number; delta: number | null };
    prsMerged: { value: number; delta: number | null };
  };
  granularity: BucketGranularity;
  buckets: Bucket[];
  rows: StatsRow[];
  legend: { personId: string; name: string; color: string }[];
};

type Range = { start: Date; end: Date };

const inRange = (range: Range, iso: string | undefined) => {
  if (!iso) return false;
  const t = Date.parse(iso);
  return t >= range.start.getTime() && t < range.end.getTime();
};

function mondayOf(date: Date): Date {
  const back = (date.getDay() + 6) % 7;
  return new Date(date.getFullYear(), date.getMonth(), date.getDate() - back);
}

const dayKeyOf = (date: Date) => localDayKey(date.toISOString());

function fraction(current: number, previous: number): number | null {
  return previous === 0 ? null : (current - previous) / previous;
}

type Totals = { commits: number; added: number; deleted: number; prs: number };

export function buildStats(input: {
  raw: RawData;
  config: ContributorsConfig;
  resolver: Resolver;
  rangeStart: Date;
  rangeEnd: Date;
  /**
   * The range the deltas compare with. Defaults to the period of equal length
   * right before `rangeStart`; null when there is nothing before (no deltas,
   * no newcomers).
   */
  previous?: Range | null;
  /** Chart buckets: calendar weeks (default), days or calendar months. */
  granularity?: BucketGranularity;
  /** Shared person colors; an id missing from it gets its hashed color. */
  colors?: Record<string, string>;
}): Stats {
  const { raw, config, resolver } = input;
  const granularity = input.granularity ?? "week";
  const range: Range = { start: input.rangeStart, end: input.rangeEnd };
  const length = range.end.getTime() - range.start.getTime();
  const previous: Range | null =
    input.previous === undefined
      ? {
          start: new Date(range.start.getTime() - length),
          end: range.start,
        }
      : input.previous;
  // Rule: every commit count here is of commits credited to a person, so the
  // KPI total equals the sum of the contributors table. Commits nobody can be
  // credited for (agent work with no human) are left out everywhere.
  const commits = attributeCommits({ raw, config, resolver }).filter(
    (c) => c.personId !== null,
  );
  const prs = attributePrs({ raw, config, resolver });

  const periodOf = (r: Range): Totals => {
    const period: Totals = { commits: 0, added: 0, deleted: 0, prs: 0 };
    for (const c of commits) {
      if (!inRange(r, c.commit.committedAt)) continue;
      period.commits += 1;
      for (const f of c.files) {
        period.added += f.added;
        period.deleted += f.deleted;
      }
    }
    period.prs = prs.filter(
      ({ pr }) => pr.state === "merged" && inRange(r, pr.mergedAt),
    ).length;
    return period;
  };

  const now = periodOf(range);
  const earlier: Totals = previous
    ? periodOf(previous)
    : { commits: 0, added: 0, deleted: 0, prs: 0 };
  const previousCovered =
    previous !== null && Date.parse(raw.since) <= previous.start.getTime();

  const inside = commits.filter((c) => inRange(range, c.commit.committedAt));
  const byPerson = new Map<string, AttributedCommit[]>();
  for (const c of inside) {
    if (!c.personId) continue;
    const list = byPerson.get(c.personId);
    if (list) list.push(c);
    else byPerson.set(c.personId, [c]);
  }

  const rows = buildRows(byPerson, resolver, config, input.colors ?? {});
  const total = rows.reduce((sum, r) => sum + r.commits, 0);
  for (const row of rows) row.sharePct = (row.commits / total) * 100;

  const firstCommit = new Map<string, number>();
  for (const c of commits) {
    if (!c.personId) continue;
    const t = Date.parse(c.commit.committedAt);
    const cur = firstCommit.get(c.personId);
    if (cur === undefined || t < cur) firstCommit.set(c.personId, t);
  }
  // Same rule as the leaderboard: the first activity in the data is inside
  // the range and the data reaches back to the start of the previous period.
  const newcomers = previousCovered
    ? rows.filter(
        (r) => (firstCommit.get(r.personId) ?? 0) >= range.start.getTime(),
      ).length
    : 0;

  return {
    kpis: {
      commits: {
        value: now.commits,
        delta: previousCovered ? fraction(now.commits, earlier.commits) : null,
      },
      contributors: {
        value: rows.length,
        identities: new Set(rows.flatMap((r) => r.identities)).size,
        newcomers,
      },
      lines: {
        added: now.added,
        deleted: now.deleted,
        delta: previousCovered
          ? fraction(now.added + now.deleted, earlier.added + earlier.deleted)
          : null,
      },
      prsMerged: {
        value: now.prs,
        delta: previousCovered ? now.prs - earlier.prs : null,
      },
    },
    granularity,
    buckets: buildBuckets(inside, range, granularity),
    rows,
    legend: rows.map((r) => ({
      personId: r.personId,
      name: r.name,
      color: r.color,
    })),
  };
}

function buildRows(
  byPerson: Map<string, AttributedCommit[]>,
  resolver: Resolver,
  config: ContributorsConfig,
  colors: Record<string, string>,
): StatsRow[] {
  const rows = Array.from(byPerson, ([personId, list]): StatsRow => {
    let added = 0;
    let deleted = 0;
    let ai = 0;
    let last = 0;
    const paths = new Set<string>();
    const identities = new Set<string>();
    for (const c of list) {
      added += c.files.reduce((s, f) => s + f.added, 0);
      deleted += c.files.reduce((s, f) => s + f.deleted, 0);
      for (const f of c.files) paths.add(f.path);
      if (c.aiAssisted) ai += 1;
      last = Math.max(last, Date.parse(c.commit.committedAt));
      if (isHuman(config, c.commit.author)) {
        identities.add(identityKey(c.commit.author));
      }
    }
    return {
      personId,
      name: resolver.nameOf(personId),
      commits: list.length,
      added,
      deleted,
      files: paths.size,
      aiAssistedPct: Math.round((ai / list.length) * 100),
      sharePct: 0,
      lastActive: last > 0 ? new Date(last).toISOString() : null,
      identities: Array.from(identities).sort(),
      color: "",
    };
  });
  rows.sort(
    (a, b) =>
      b.commits - a.commits ||
      a.name.localeCompare(b.name) ||
      a.personId.localeCompare(b.personId),
  );
  for (const row of rows) row.color = colorFromMap(colors, row.personId);
  return rows;
}

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

function bucketAt(start: Date, granularity: BucketGranularity): Bucket {
  const month = MONTHS[start.getMonth()];
  const year = start.getFullYear();
  const day = start.getDate();
  if (granularity === "year") {
    return {
      start: dayKeyOf(start),
      label: String(year),
      short: String(year),
      byPerson: {},
    };
  }
  if (granularity === "month") {
    return {
      start: dayKeyOf(start),
      label: `${month} ${year}`,
      short: `${month} ${year}`,
      byPerson: {},
    };
  }
  return {
    start: dayKeyOf(start),
    label:
      granularity === "day"
        ? `${month} ${day}, ${year}`
        : `Week of ${month} ${day}, ${year}`,
    short: `${month} ${day}`,
    byPerson: {},
  };
}

function buildBuckets(
  inside: AttributedCommit[],
  range: Range,
  granularity: BucketGranularity,
): Bucket[] {
  const startOf = (date: Date) => {
    if (granularity === "year") return new Date(date.getFullYear(), 0, 1);
    if (granularity === "month") {
      return new Date(date.getFullYear(), date.getMonth(), 1);
    }
    return granularity === "day"
      ? new Date(date.getFullYear(), date.getMonth(), date.getDate())
      : mondayOf(date);
  };
  const next = (date: Date) => {
    if (granularity === "year") return new Date(date.getFullYear() + 1, 0, 1);
    if (granularity === "month") {
      return new Date(date.getFullYear(), date.getMonth() + 1, 1);
    }
    return new Date(
      date.getFullYear(),
      date.getMonth(),
      date.getDate() + (granularity === "day" ? 1 : 7),
    );
  };
  const buckets = new Map<string, Bucket>();
  for (
    let b = startOf(range.start);
    b.getTime() < range.end.getTime();
    b = next(b)
  ) {
    buckets.set(dayKeyOf(b), bucketAt(b, granularity));
  }
  for (const c of inside) {
    if (!c.personId) continue;
    const bucket = buckets.get(
      dayKeyOf(startOf(new Date(c.commit.committedAt))),
    );
    if (bucket) {
      bucket.byPerson[c.personId] = (bucket.byPerson[c.personId] ?? 0) + 1;
    }
  }
  return Array.from(buckets.values());
}
