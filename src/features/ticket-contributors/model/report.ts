import { previousPeriod as previousPeriodOf } from "../../contributors/model/period";
import type { Period } from "../../contributors/model/period";
import {
  activeTickets,
  buildBoard,
  excludedMembersOf,
  periodTickets,
  statusesOf,
} from "./board";
import type { Board } from "./board";
import { CRITERIA, LEVELS, deliveredAt, effectiveLevel } from "./tickets";
import type { Criterion, Level, TicketRecord, WorkspaceScope } from "./tickets";
import type { AiConfigStatus } from "./ticketsApi";

export type ReportInput = {
  workspaceName: string;
  period: Period;
  previousPeriod: Period | null;
  scope: WorkspaceScope;
  points: Record<Level, number>;
  tickets: TicketRecord[];
  previousTickets: TicketRecord[];
  ai: AiConfigStatus | null;
  generatedAt: Date;
  /**
   * The previous Period's tickets were not fully fetched, so `previousTickets`
   * was left out; the report then says why there is no comparison.
   */
  previousIncomplete?: boolean;
};

export type ReportNote = { headline: string; text: string };

export type ReportTile = {
  id: "delivered" | "scored" | "points" | "contributors" | "active" | "median";
  label: string;
  /** Display text, already formatted. */
  value: string;
  /** Small line under the value; `null` leaves it out. */
  sub: string | null;
};

export type ReportMember = {
  id: string;
  name: string;
  points: number;
  /** Whole percent of the team's points. */
  sharePercent: number;
  delivered: number;
  scored: number;
  /** Scored tickets out of delivered tickets, 0 to 1. */
  coverage: number;
  /** Coverage as shown: whole percent, capped like the Scored tile. */
  coveragePercent: number;
  /** Delivered tickets with coverage below 50%: the totals are incomplete and drawn faded. */
  faded: boolean;
  levels: Record<Level, number>;
  /** Points per scored ticket; `null` without scored tickets. */
  avgPoints: number | null;
  active: number;
};

export type TrendUnit = "day" | "week" | "month" | "year";

export type TrendBucket = {
  label: string;
  title: string;
  points: number;
};

export type HardestTicket = {
  id: string;
  title: string;
  /** Only an `https://` URL; otherwise `null` and the title is plain text. */
  url: string | null;
  members: string;
  level: Level;
  points: number;
  edited: boolean;
  reason: string;
};

export type MatrixCell = {
  points: number;
  delivered: number;
  scored: number;
  /** 0 to 4 on the blue ramp by points against the table's busiest cell; `null` means no background. */
  shade: number | null;
};

export type MatrixRow = {
  id: string;
  name: string;
  /** The row of delivered tickets without an assignee: counts only, no points. */
  unassigned: boolean;
  /** Overall coverage below 50%: the row is drawn faded, like the overview table. */
  faded: boolean;
  cells: MatrixCell[];
  total: MatrixCell;
};

export type ReportMonth = {
  /** `2026-10`. */
  key: string;
  /** `Oct`. */
  label: string;
  /** `October 2026`. */
  title: string;
  delivered: number;
  points: number;
  tiles: ReportTile[];
  members: ReportMember[];
  unassigned: { delivered: number; scored: number } | null;
  hardest: HardestTicket[];
  /** Coverage note of this month alone; `null` at 100% or without tickets. */
  note: ReportNote | null;
};

/** Month by month view of a quarter or a year. */
export type ReportMonthly = {
  months: ReportMonth[];
  rows: MatrixRow[];
};

export type ReportData = {
  workspaceName: string;
  periodLabel: string;
  generatedLabel: string;
  /** For example `claude-sonnet-5-5 (Claude Code)`; `null` without AI config. */
  scoredWith: string | null;
  notes: ReportNote[];
  /** Why there is no comparison with the previous Period; `null` when there is none to explain. */
  comparisonNote: string | null;
  tiles: ReportTile[];
  members: ReportMember[];
  /** Delivered tickets without an assignee; `null` when there are none. */
  unassigned: { delivered: number; scored: number } | null;
  trend: { unit: TrendUnit; buckets: TrendBucket[] };
  load: { id: string; name: string; delivered: number; active: number }[];
  hardest: HardestTicket[];
  /** Only for quarter and year periods. */
  monthly: ReportMonthly | null;
  statuses: { delivered: string[]; excluded: string[] };
  /** Names of the members left off the board; empty when nobody is. */
  excludedMembers: string[];
  points: Record<Level, number>;
};

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

// Series, level ramp and surfaces. The same values feed the CSS and the SVG
// attributes, which cannot rely on CSS variables.
const COLOR = {
  page: "#f9f9f7",
  surface: "#fcfcfb",
  textPrimary: "#0b0b0b",
  textSecondary: "#52514e",
  textMuted: "#898781",
  grid: "#e1e0d9",
  axis: "#c3c2b7",
  border: "rgba(11, 11, 11, 0.10)",
  series1: "#2a78d6",
  series2: "#eb6834",
  warnBg: "#fff6e0",
  warnInk: "#7a5300",
  /** Muted ink for text on a shaded matrix cell: #52514e falls below 4.5:1 on the darker steps. */
  inkOnShade: "#2b2a28",
} as const;

const MONTH_NAMES = [
  "January",
  "February",
  "March",
  "April",
  "May",
  "June",
  "July",
  "August",
  "September",
  "October",
  "November",
  "December",
];

/** Light steps of the level ramp: backgrounds under dark ink in the month matrix. */
const CELL_RAMP = ["#cde2fb", "#b7d3f6", "#9ec5f4", "#86b6ef", "#6da7ec"];

const LEVEL_COLOR: Record<Level, string> = {
  XS: "#86b6ef",
  S: "#5598e7",
  M: "#2a78d6",
  L: "#1c5cab",
  XL: "#104281",
};

const FADE_BELOW = 0.5;
const HARDEST_LIMIT = 10;
const MONTH_HARDEST_LIMIT = 3;

// ---------------------------------------------------------------- formatting

/** Escapes text for HTML content and attribute values. */
function esc(value: string): string {
  return value
    .replace(/&/g, "&amp;")
    .replace(/</g, "&lt;")
    .replace(/>/g, "&gt;")
    .replace(/"/g, "&quot;")
    .replace(/'/g, "&#39;");
}

/** Whole numbers stay whole; a split share keeps one decimal. */
function fmt(n: number): string {
  return (Math.round(n * 10) / 10).toLocaleString("en-US", {
    maximumFractionDigits: 1,
  });
}

/**
 * Rounded percent that never crosses a boundary by rounding: below 100% it
 * stays at 99 or less, below 50% at 49 or less, and above 0 it is at least 1.
 */
function percent(part: number, whole: number): number {
  if (whole <= 0 || part <= 0) return 0;
  if (part >= whole) return 100;
  const rounded = Math.max(1, Math.round((part / whole) * 100));
  return Math.min(part / whole < FADE_BELOW ? 49 : 99, rounded);
}

const dayLabel = (d: Date) => `${d.getDate()} ${MONTHS[d.getMonth()]}`;

/** Compact number for SVG attributes. */
const num = (n: number) => String(Number(n.toFixed(2)));

function clip(text: string, max: number): string {
  return text.length > max ? `${text.slice(0, Math.max(1, max - 3))}...` : text;
}

function runnerLabel(runner: AiConfigStatus["runner"]): string {
  return runner === "claude-code" ? "Claude Code" : "API";
}

// --------------------------------------------------------------------- build

function slug(text: string): string {
  const s = text
    .toLowerCase()
    .replace(/[^a-z0-9]+/g, "-")
    .replace(/^-+|-+$/g, "");
  return s === "" ? "workspace" : s;
}

export function reportFileName(workspaceName: string, period: Period): string {
  return `ticket-contributors-${slug(workspaceName)}-${period.id}.html`;
}

const addDays = (d: Date, days: number) =>
  new Date(d.getFullYear(), d.getMonth(), d.getDate() + days);
const monthStart = (y: number, m: number) => new Date(y, m, 1);

/** Monday 00:00 of the local week that contains `d`. */
const mondayOf = (d: Date) => addDays(d, -((d.getDay() + 6) % 7));

type BucketPlan = { unit: TrendUnit; starts: Date[] };

function monthRange(from: Date, endExclusive: Date): Date[] {
  const starts: Date[] = [];
  for (
    let d = monthStart(from.getFullYear(), from.getMonth());
    d.getTime() < endExclusive.getTime();
    d = monthStart(d.getFullYear(), d.getMonth() + 1)
  ) {
    starts.push(d);
  }
  return starts;
}

/**
 * Buckets for the trend: days for 7d, ISO weeks for a month, months for a
 * quarter or a year; All time uses months up to 24 of them, else years.
 */
function planBuckets(period: Period, delivered: TicketRecord[]): BucketPlan {
  switch (period.kind) {
    case "7d": {
      const starts: Date[] = [];
      for (
        let d = period.start;
        d.getTime() < period.end.getTime();
        d = addDays(d, 1)
      ) {
        starts.push(d);
      }
      return { unit: "day", starts };
    }
    case "month": {
      const starts: Date[] = [];
      for (
        let d = mondayOf(period.start);
        d.getTime() < period.end.getTime();
        d = addDays(d, 7)
      ) {
        starts.push(d);
      }
      return { unit: "week", starts };
    }
    case "quarter":
    case "year":
      return {
        unit: "month",
        starts: monthRange(period.start, period.end),
      };
    case "all": {
      const times = delivered
        .map((t) => Date.parse(deliveredAt(t).at))
        .filter((t) => Number.isFinite(t));
      if (times.length === 0) return { unit: "month", starts: [] };
      const first = new Date(Math.min(...times));
      const last = new Date(Math.max(...times));
      const months =
        (last.getFullYear() - first.getFullYear()) * 12 +
        last.getMonth() -
        first.getMonth() +
        1;
      if (months <= 24) {
        return {
          unit: "month",
          starts: monthRange(
            first,
            monthStart(last.getFullYear(), last.getMonth() + 1),
          ),
        };
      }
      const starts: Date[] = [];
      for (let y = first.getFullYear(); y <= last.getFullYear(); y += 1) {
        starts.push(new Date(y, 0, 1));
      }
      return { unit: "year", starts };
    }
  }
}

function bucketLabels(
  unit: TrendUnit,
  start: Date,
  period: Period,
): { label: string; title: string } {
  switch (unit) {
    case "day":
      return { label: dayLabel(start), title: dayLabel(start) };
    case "week":
      return { label: dayLabel(start), title: `Week of ${dayLabel(start)}` };
    case "month": {
      const month = MONTHS[start.getMonth()];
      const full = `${month} ${start.getFullYear()}`;
      return { label: period.kind === "all" ? full : month, title: full };
    }
    case "year":
      return {
        label: String(start.getFullYear()),
        title: String(start.getFullYear()),
      };
  }
}

function median(values: number[]): number | null {
  if (values.length === 0) return null;
  const sorted = [...values].sort((a, b) => a - b);
  const mid = Math.floor(sorted.length / 2);
  return sorted.length % 2 === 1
    ? sorted[mid]
    : (sorted[mid - 1] + sorted[mid]) / 2;
}

/** Reason of the highest-scoring criterion; criteria order breaks ties. */
function mainReason(t: TicketRecord): string {
  if (t.overrideLevel) {
    const note = t.overrideNote?.trim();
    return note ? note : "Set by hand";
  }
  if (!t.assessment) return "";
  let best: Criterion = CRITERIA[0];
  for (const key of CRITERIA) {
    if (t.assessment.criteria[key] > t.assessment.criteria[best]) best = key;
  }
  return t.assessment.reasons[best] ?? "";
}

const deliveredTime = (t: TicketRecord) => {
  const time = Date.parse(deliveredAt(t).at);
  return Number.isNaN(time) ? 0 : time;
};

function distinctNames(t: TicketRecord): string[] {
  const seen = new Map<string, string>();
  for (const a of t.assignees) if (!seen.has(a.id)) seen.set(a.id, a.name);
  return [...seen.values()];
}

function unassignedOf(board: Board): ReportData["unassigned"] {
  return board.unassigned.tickets > 0
    ? { delivered: board.unassigned.tickets, scored: board.unassigned.assessed }
    : null;
}

/** Index of the last start at or before `time`; -1 before the first. */
function bucketIndex(starts: Date[], time: number): number {
  let index = -1;
  for (let i = 0; i < starts.length; i += 1) {
    if (starts[i].getTime() <= time) index = i;
    else break;
  }
  return index;
}

function reportMembers(board: Board): {
  members: ReportMember[];
  teamPoints: number;
} {
  const teamPoints = board.members.reduce((sum, m) => sum + m.points, 0);
  const members: ReportMember[] = board.members.map((m) => {
    const coverage = m.tickets > 0 ? m.assessed / m.tickets : 0;
    return {
      id: m.id,
      name: m.name,
      points: m.points,
      sharePercent:
        teamPoints > 0 ? Math.round((m.points / teamPoints) * 100) : 0,
      delivered: m.tickets,
      scored: m.assessed,
      coverage,
      coveragePercent: percent(m.assessed, m.tickets),
      // Coverage means nothing for a member with no delivered tickets.
      faded: m.tickets > 0 && coverage < FADE_BELOW,
      levels: { ...m.levels },
      avgPoints: m.assessed > 0 ? m.points / m.assessed : null,
      active: m.active,
    };
  });
  return { members, teamPoints };
}

const scoredTile = (board: Board): ReportTile => ({
  id: "scored",
  label: "Scored",
  value: board.total > 0 ? `${percent(board.assessed, board.total)}%` : "-",
  sub: `${board.assessed} of ${board.total}`,
});

const pointsTile = (teamPoints: number): ReportTile => ({
  id: "points",
  label: "Team points",
  value: fmt(teamPoints),
  sub: "scored tickets only",
});

const activeTile = (count: number): ReportTile => ({
  id: "active",
  label: "Active tickets",
  value: String(count),
  sub: "in a working status",
});

/** Coverage note when some delivered tickets are not scored; `where` narrows it to a span. */
function coverageNote(board: Board, where = ""): ReportNote | null {
  if (!(board.total > 0 && board.assessed < board.total)) return null;
  return {
    headline: `Scoring coverage is ${percent(board.assessed, board.total)}%.`,
    text: `${board.assessed} of ${board.total} delivered tickets${where} are scored. Faded members have less than 50% of their tickets scored, so their totals are incomplete.`,
  };
}

/** Delivered and scored tickets, by points then newest delivery. */
function hardestOf(
  delivered: TicketRecord[],
  points: Record<Level, number>,
  limit: number,
): HardestTicket[] {
  return delivered
    .flatMap((t) => {
      const level = effectiveLevel(t);
      return level ? [{ t, level }] : [];
    })
    .map(({ t, level }) => ({
      t,
      level,
      worth: points[level],
      time: deliveredTime(t),
    }))
    .sort(
      (a, b) =>
        b.worth - a.worth || b.time - a.time || a.t.id.localeCompare(b.t.id),
    )
    .slice(0, limit)
    .map(({ t, level, worth }) => {
      const names = distinctNames(t);
      return {
        id: t.id,
        title: t.title,
        url: t.url.startsWith("https://") ? t.url : null,
        members: names.length > 0 ? names.join(", ") : "Unassigned",
        level,
        points: worth,
        edited: t.overrideLevel !== null,
        reason: mainReason(t),
      };
    });
}

/**
 * One month per column of a quarter or a year, up to the month of
 * `generatedAt`. A ticket belongs to the month of its delivery date (an active
 * one, of its update date); one dated after the last month joins it, as in
 * the trend. Rows follow the overview member table.
 */
function buildMonthly(
  period: Period,
  delivered: TicketRecord[],
  active: TicketRecord[],
  overview: { members: ReportMember[]; unassigned: ReportData["unassigned"] },
  points: Record<Level, number>,
  generatedAt: Date,
): ReportMonthly {
  const starts = monthRange(period.start, period.end).filter(
    (start) => start.getTime() <= generatedAt.getTime(),
  );
  const deliveredBy = starts.map(() => [] as TicketRecord[]);
  const activeBy = starts.map(() => [] as TicketRecord[]);
  for (const t of delivered) {
    const i = bucketIndex(starts, deliveredTime(t));
    if (i >= 0) deliveredBy[i].push(t);
  }
  for (const t of active) {
    const i = bucketIndex(starts, Date.parse(t.updatedAt));
    if (i >= 0) activeBy[i].push(t);
  }

  const months: ReportMonth[] = starts.map((start, i) => {
    const board = buildBoard(deliveredBy[i], points, activeBy[i]);
    const { members, teamPoints } = reportMembers(board);
    const name = MONTH_NAMES[start.getMonth()];
    const title = `${name} ${start.getFullYear()}`;
    return {
      key: `${start.getFullYear()}-${String(start.getMonth() + 1).padStart(2, "0")}`,
      label: MONTHS[start.getMonth()],
      title,
      delivered: board.total,
      points: teamPoints,
      tiles: [
        {
          id: "delivered",
          label: "Delivered tickets",
          value: String(board.total),
          sub: null,
        },
        scoredTile(board),
        pointsTile(teamPoints),
        activeTile(activeBy[i].length),
      ],
      members,
      unassigned: unassignedOf(board),
      hardest: hardestOf(deliveredBy[i], points, MONTH_HARDEST_LIMIT),
      note: coverageNote(board, ` in ${title}`),
    };
  });

  const cellOf = (c: Omit<MatrixCell, "shade">): MatrixCell => ({
    ...c,
    shade: null,
  });
  const rows: MatrixRow[] = overview.members.map((m) => ({
    id: m.id,
    name: m.name,
    unassigned: false,
    faded: m.faded,
    cells: months.map((month) => {
      const own = month.members.find((x) => x.id === m.id);
      return cellOf({
        points: own?.points ?? 0,
        delivered: own?.delivered ?? 0,
        scored: own?.scored ?? 0,
      });
    }),
    total: cellOf({
      points: m.points,
      delivered: m.delivered,
      scored: m.scored,
    }),
  }));
  if (overview.unassigned) {
    rows.push({
      id: "",
      name: "Unassigned",
      unassigned: true,
      faded: false,
      cells: months.map((month) =>
        cellOf({
          points: 0,
          delivered: month.unassigned?.delivered ?? 0,
          scored: month.unassigned?.scored ?? 0,
        }),
      ),
      total: cellOf({
        points: 0,
        delivered: overview.unassigned.delivered,
        scored: overview.unassigned.scored,
      }),
    });
  }

  // Shade by points against the busiest month cell of any member.
  const max = Math.max(
    0,
    ...rows
      .filter((r) => !r.unassigned)
      .flatMap((r) => r.cells.map((c) => c.points)),
  );
  for (const row of rows) {
    for (const cell of row.cells) {
      if (max > 0 && cell.points > 0) {
        cell.shade = Math.min(
          CELL_RAMP.length - 1,
          Math.max(0, Math.ceil((cell.points / max) * CELL_RAMP.length) - 1),
        );
      }
    }
  }
  return { months, rows };
}

export function buildReport(input: ReportInput): ReportData {
  const { period, scope, points, generatedAt } = input;
  const delivered = periodTickets(input.tickets, scope, period);
  const active = activeTickets(input.tickets, scope, period);
  const board = buildBoard(delivered, points, active);

  const { members, teamPoints } = reportMembers(board);
  const unassigned = unassignedOf(board);

  // Comparison with the previous Period: not for All time, nor from zero. A
  // Period still running is compared with the same elapsed span of the
  // previous one; the rolling 7 days and a finished Period compare whole.
  const before = input.previousPeriod ?? previousPeriodOf(period);
  let comparison: string | null = null;
  let comparisonNote: string | null = null;
  if (period.kind !== "all" && before && input.previousIncomplete) {
    const label = before.kind === "7d" ? "the previous 7 days" : before.label;
    comparisonNote = `No comparison with ${label}: its tickets were not fully fetched. Open that period in Ticket Contributors once to include it.`;
  } else if (period.kind !== "all" && before) {
    const running =
      period.kind !== "7d" && generatedAt.getTime() < period.end.getTime();
    let against = before.kind === "7d" ? "the previous 7 days" : before.label;
    let span: Period = before;
    if (running) {
      const elapsed = generatedAt.getTime() - period.start.getTime();
      const end = Math.min(
        before.end.getTime(),
        before.start.getTime() + Math.max(0, elapsed),
      );
      span = { ...before, end: new Date(end) };
      against = `the same days of ${before.label}`;
    }
    const previous = periodTickets(input.previousTickets, scope, span).length;
    if (previous > 0) {
      const change = Math.round(((board.total - previous) / previous) * 100);
      comparison = `${change > 0 ? "+" : ""}${change}% vs ${against}`;
    }
  }

  const scoredMedian = median(
    members.filter((m) => m.scored > 0).map((m) => m.points),
  );
  const tiles: ReportTile[] = [
    {
      id: "delivered",
      label: "Delivered tickets",
      value: String(board.total),
      sub: comparison,
    },
    scoredTile(board),
    pointsTile(teamPoints),
    {
      id: "contributors",
      label: "Contributors",
      value: String(members.filter((m) => m.delivered > 0).length),
      sub: "with delivered work",
    },
    activeTile(active.length),
    {
      id: "median",
      label: "Median points per person",
      value: scoredMedian === null ? "-" : fmt(scoredMedian),
      sub: "among scored work",
    },
  ];

  const notes: ReportNote[] = [];
  const coverage = coverageNote(board);
  if (coverage) notes.push(coverage);
  if (board.datesApproximate) {
    notes.push({
      headline: "Some delivery dates are approximate.",
      text: "Tickets without a done date or status history use their last update date.",
    });
  }

  // Trend: team points (member shares) by delivery date.
  const planned = planBuckets(period, delivered);
  // Buckets that start after the report was generated are still to come.
  const plan: BucketPlan = {
    unit: planned.unit,
    starts: planned.starts.filter(
      (start) => start.getTime() <= generatedAt.getTime(),
    ),
  };
  const totals = plan.starts.map(() => 0);
  for (const member of board.members) {
    for (const entry of member.ticketList) {
      const index = bucketIndex(plan.starts, deliveredTime(entry.ticket));
      if (index >= 0) totals[index] += entry.share;
    }
  }
  const buckets: TrendBucket[] = plan.starts.map((start, i) => ({
    ...bucketLabels(plan.unit, start, period),
    points: totals[i],
  }));

  // Hardest tickets: delivered and scored, by points then newest delivery.
  const hardest = hardestOf(delivered, points, HARDEST_LIMIT);

  // Delivered statuses seen in the period plus those chosen in the scope.
  const deliveredNames = statusesOf(delivered).map((s) => s.name);
  const known = new Set(deliveredNames.map((n) => n.toLowerCase()));
  for (const name of scope.deliveredStatuses) {
    const trimmed = name.trim();
    if (trimmed !== "" && !known.has(trimmed.toLowerCase())) {
      known.add(trimmed.toLowerCase());
      deliveredNames.push(trimmed);
    }
  }

  return {
    workspaceName: input.workspaceName,
    periodLabel: period.label,
    generatedLabel: `${generatedAt.getDate()} ${MONTHS[generatedAt.getMonth()]} ${generatedAt.getFullYear()}`,
    scoredWith: input.ai
      ? `${input.ai.model} (${runnerLabel(input.ai.runner)})`
      : null,
    notes,
    comparisonNote,
    tiles,
    members,
    unassigned,
    trend: { unit: plan.unit, buckets },
    load: members.map((m) => ({
      id: m.id,
      name: m.name,
      delivered: m.delivered,
      active: m.active,
    })),
    hardest,
    monthly:
      period.kind === "quarter" || period.kind === "year"
        ? buildMonthly(
            period,
            delivered,
            active,
            { members, unassigned },
            points,
            generatedAt,
          )
        : null,
    statuses: {
      delivered: deliveredNames,
      excluded: scope.excludedStatuses
        .map((s) => s.trim())
        .filter((s) => s !== ""),
    },
    excludedMembers: excludedMembersOf(input.tickets, scope).map((m) => m.name),
    points: { ...points },
  };
}

// ----------------------------------------------------------------- SVG parts

const W = 460;
const ROW = 30;
const BAR = 18;

/** Rounded data end only, square at the baseline. */
function hbarPath(x: number, y: number, w: number, h: number, r: number) {
  const rr = Math.min(r, w, h / 2);
  return `M${num(x)},${num(y)}H${num(x + w - rr)}Q${num(x + w)},${num(y)} ${num(x + w)},${num(y + rr)}V${num(y + h - rr)}Q${num(x + w)},${num(y + h)} ${num(x + w - rr)},${num(y + h)}H${num(x)}Z`;
}

function vbarPath(x: number, y: number, w: number, h: number, r: number) {
  const rr = Math.min(r, h, w / 2);
  return `M${num(x)},${num(y + h)}V${num(y + rr)}Q${num(x)},${num(y)} ${num(x + rr)},${num(y)}H${num(x + w - rr)}Q${num(x + w)},${num(y)} ${num(x + w)},${num(y + rr)}V${num(y + h)}Z`;
}

/** Axis ticks from 0 up to a round value at or above `max`. */
function niceTicks(max: number, target = 4): number[] {
  if (!(max > 0)) return [0, 1];
  const raw = Math.max(1, max / target);
  const pow = Math.pow(10, Math.floor(Math.log10(raw)));
  const step =
    [1, 2, 5, 10].map((k) => k * pow).find((s) => s >= raw) ?? 10 * pow;
  const ticks: number[] = [];
  for (let v = 0; v < max + step; v += step) {
    ticks.push(v);
    if (v >= max) break;
  }
  return ticks;
}

const svgOpen = (h: number, label: string) =>
  `<svg viewBox="0 0 ${W} ${h}" width="100%" role="img" aria-label="${esc(label)}">`;

const empty = (text: string) => `<p class="empty">${esc(text)}</p>`;

/** Characters of a member name that fit the 100px left margin of the bar charts. */
const NAME_MAX = 14;

function nameText(x: number, y: number, name: string, max: number): string {
  const clipped = clip(name, max);
  const title = clipped === name ? "" : `<title>${esc(name)}</title>`;
  return `<text x="${x}" y="${y}" text-anchor="end">${esc(clipped)}${title}</text>`;
}

function pointsChart(members: ReportMember[]): string {
  if (members.length === 0)
    return empty("No delivered tickets in this period.");
  const L = 100;
  const R = members.some((m) => m.faded && m.points > 0) ? 118 : 48;
  const H = members.length * ROW + 8;
  const max = Math.max(...members.map((m) => m.points)) || 1;
  const out = [
    svgOpen(H, "Points by member, horizontal bars"),
    `<line x1="${L}" x2="${L}" y1="0" y2="${H - 4}" stroke="${COLOR.axis}"/>`,
  ];
  members.forEach((m, i) => {
    const y = i * ROW + 6;
    const w = ((W - L - R) * m.points) / max;
    const pct = m.coveragePercent;
    out.push(nameText(L - 8, y + 14, m.name, NAME_MAX));
    if (m.points > 0) {
      out.push(
        `<path d="${hbarPath(L, y, w, BAR, 4)}" fill="${COLOR.series1}"${m.faded ? ' opacity="0.35"' : ""}/>`,
        `<rect x="${L}" y="${y - 4}" width="${W - L}" height="26" fill="transparent"><title>${esc(`${m.name}: ${fmt(m.points)} points, coverage ${pct}%`)}</title></rect>`,
      );
    }
    const label =
      m.delivered === 0
        ? "no delivered work"
        : m.points > 0
          ? fmt(m.points) + (m.faded ? ` (${pct}% scored)` : "")
          : "not scored yet";
    out.push(
      `<text x="${num(L + w + 6)}" y="${y + 14}" class="${m.faded || m.delivered === 0 ? "muted" : "val"}">${esc(label)}</text>`,
    );
  });
  out.push("</svg>");
  return out.join("");
}

function levelsChart(members: ReportMember[]): string {
  const scored = members.filter((m) => m.scored > 0);
  if (scored.length === 0) return empty("No scored tickets yet.");
  const L = 100;
  const R = 30;
  const H = scored.length * ROW + 8;
  const totalOf = (m: ReportMember) =>
    LEVELS.reduce((sum, l) => sum + m.levels[l], 0);
  const max = Math.max(...scored.map(totalOf)) || 1;
  const out = [
    svgOpen(H, "Scored tickets per level per member, stacked bars"),
    `<line x1="${L}" x2="${L}" y1="0" y2="${H - 4}" stroke="${COLOR.axis}"/>`,
  ];
  scored.forEach((m, i) => {
    const y = i * ROW + 6;
    let x = L;
    out.push(nameText(L - 8, y + 14, m.name, NAME_MAX));
    const segments = LEVELS.filter((l) => m.levels[l] > 0);
    segments.forEach((level, j) => {
      const n = m.levels[level];
      const w = ((W - L - R) * n) / max;
      const title = `<title>${esc(`${m.name}: ${level} ${n} ${n === 1 ? "ticket" : "tickets"}`)}</title>`;
      if (j === segments.length - 1) {
        out.push(
          `<path d="${hbarPath(x, y, w, BAR, 4)}" fill="${LEVEL_COLOR[level]}">${title}</path>`,
        );
      } else {
        out.push(
          `<rect x="${num(x)}" y="${y}" width="${num(Math.max(0, w - 2))}" height="${BAR}" fill="${LEVEL_COLOR[level]}">${title}</rect>`,
        );
      }
      x += w;
    });
    out.push(
      `<text x="${num(x + 6)}" y="${y + 14}" class="val">${totalOf(m)}</text>`,
    );
  });
  out.push("</svg>");
  return out.join("");
}

function trendChart(trend: ReportData["trend"]): string {
  const { buckets } = trend;
  if (buckets.length === 0)
    return empty("No delivered tickets in this period.");
  const H = 220;
  const L = 40;
  const R = 26;
  const T = 14;
  const B = 28;
  const ticks = niceTicks(Math.max(...buckets.map((b) => b.points)));
  const top = ticks[ticks.length - 1];
  const yOf = (v: number) => T + (H - T - B) * (1 - v / top);
  const plotW = W - L - R;
  const step = buckets.length > 1 ? plotW / (buckets.length - 1) : plotW;
  const xOf = (i: number) =>
    buckets.length > 1 ? L + i * step : L + plotW / 2;
  const out = [svgOpen(H, `Team points per ${trend.unit}, line chart`)];
  for (const v of ticks) {
    out.push(
      `<line x1="${L}" x2="${W - R}" y1="${num(yOf(v))}" y2="${num(yOf(v))}" stroke="${v === 0 ? COLOR.axis : COLOR.grid}"/>`,
      `<text x="${L - 6}" y="${num(yOf(v) + 4)}" text-anchor="end" class="muted">${fmt(v)}</text>`,
    );
  }
  const coords = buckets.map((b, i) => `${num(xOf(i))},${num(yOf(b.points))}`);
  if (buckets.length > 1) {
    out.push(
      `<path d="M${coords.join("L")}" fill="none" stroke="${COLOR.series1}" stroke-width="2" stroke-linejoin="round"/>`,
    );
  }
  // Skip labels when they would run into each other.
  const widest = Math.max(...buckets.map((b) => b.label.length));
  const every = Math.max(1, Math.ceil((widest * 7 + 8) / step));
  buckets.forEach((b, i) => {
    const x = xOf(i);
    if (i % every === 0) {
      out.push(
        `<text x="${num(x)}" y="${H - 8}" text-anchor="middle" class="muted">${esc(b.label)}</text>`,
      );
    }
    out.push(
      `<circle cx="${num(x)}" cy="${num(yOf(b.points))}" r="4" fill="${COLOR.series1}" stroke="${COLOR.surface}" stroke-width="2"/>`,
      `<rect x="${num(x - (buckets.length > 1 ? step / 2 : plotW / 2))}" y="${T}" width="${num(buckets.length > 1 ? step : plotW)}" height="${H - T - B}" fill="transparent"><title>${esc(`${b.title}: ${fmt(b.points)} points`)}</title></rect>`,
    );
  });
  const lastIndex = buckets.length - 1;
  const lastY = yOf(buckets[lastIndex].points);
  out.push(
    `<text x="${num(xOf(lastIndex) - 4)}" y="${num(lastY - 10 < T ? lastY + 18 : lastY - 10)}" text-anchor="end" class="val">${fmt(buckets[lastIndex].points)}</text>`,
  );
  out.push("</svg>");
  return out.join("");
}

function loadChart(load: ReportData["load"]): string {
  if (load.length === 0) return empty("No work to show in this period.");
  const H = 220;
  const L = 30;
  const R = 8;
  const T = 14;
  const B = 34;
  const ticks = niceTicks(
    Math.max(...load.flatMap((m) => [m.delivered, m.active])),
    2,
  );
  const top = ticks[ticks.length - 1];
  const yOf = (v: number) => T + (H - T - B) * (1 - v / top);
  const band = (W - L - R) / load.length;
  const bw = Math.max(2, Math.min(14, (band - 6) / 2));
  const out = [
    svgOpen(H, "Delivered and active tickets per member, grouped bars"),
  ];
  for (const v of ticks) {
    out.push(
      `<line x1="${L}" x2="${W - R}" y1="${num(yOf(v))}" y2="${num(yOf(v))}" stroke="${v === 0 ? COLOR.axis : COLOR.grid}"/>`,
      `<text x="${L - 6}" y="${num(yOf(v) + 4)}" text-anchor="end" class="muted">${fmt(v)}</text>`,
    );
  }
  const maxChars = Math.max(2, Math.floor(band / 6.5));
  load.forEach((m, i) => {
    const cx = L + band * i + band / 2;
    const bars: [string, number, string, number][] = [
      ["Delivered", m.delivered, COLOR.series1, -bw - 1],
      ["Active", m.active, COLOR.series2, 1],
    ];
    for (const [label, value, color, dx] of bars) {
      if (value <= 0) continue;
      const y = yOf(value);
      out.push(
        `<path d="${vbarPath(cx + dx, y, bw, yOf(0) - y, 4)}" fill="${color}"><title>${esc(`${m.name}: ${label} ${value} ${value === 1 ? "ticket" : "tickets"}`)}</title></path>`,
      );
    }
    const first = m.name.trim().split(/\s+/)[0] ?? m.name;
    out.push(
      `<text x="${num(cx)}" y="${H - 16}" text-anchor="middle" class="muted">${esc(clip(first, maxChars))}<title>${esc(m.name)}</title></text>`,
    );
  });
  out.push("</svg>");
  return out.join("");
}

// ---------------------------------------------------------------------- HTML

const CSS = `
  .viz-root {
    color-scheme: light;
    --page: ${COLOR.page};
    --surface-1: ${COLOR.surface};
    --text-primary: ${COLOR.textPrimary};
    --text-secondary: ${COLOR.textSecondary};
    --text-muted: ${COLOR.textMuted};
    --grid: ${COLOR.grid};
    --axis: ${COLOR.axis};
    --border: ${COLOR.border};
    --series-1: ${COLOR.series1};
    --series-2: ${COLOR.series2};
    --warn-bg: ${COLOR.warnBg};
    --warn-ink: ${COLOR.warnInk};
    --ink-on-shade: ${COLOR.inkOnShade};
  }
  * { box-sizing: border-box; }
  body { margin: 0; background: var(--page); color: var(--text-primary);
    font: 14px/1.5 system-ui, -apple-system, "Segoe UI", sans-serif; }
  .page { max-width: 1040px; margin: 0 auto; padding: 32px 24px 64px; }
  header h1 { font-size: 22px; margin: 0 0 4px; }
  header .meta { color: var(--text-secondary); font-size: 13px; }
  .notes { display: flex; flex-direction: column; gap: 6px; margin: 16px 0 24px; }
  .note { background: var(--warn-bg); color: var(--warn-ink); border-radius: 8px; padding: 8px 12px; font-size: 13px; }
  .note b::before { content: "\\26A0  "; }
  .tiles { display: grid; grid-template-columns: repeat(auto-fit, minmax(150px, 1fr)); gap: 12px; margin: 24px 0; }
  .notes + .tiles { margin-top: 0; }
  .tile { background: var(--surface-1); border: 1px solid var(--border); border-radius: 12px; padding: 14px 16px; }
  .tile .label { color: var(--text-secondary); font-size: 12px; }
  .tile .value { font-size: 26px; font-weight: 600; margin-top: 2px; }
  .tile .sub { color: var(--text-muted); font-size: 12px; }
  .grid2 { display: grid; grid-template-columns: 1fr 1fr; gap: 16px; }
  @media (max-width: 820px) { .grid2 { grid-template-columns: 1fr; } }
  .card { background: var(--surface-1); border: 1px solid var(--border); border-radius: 12px; padding: 16px; margin-bottom: 16px; }
  .card h2 { font-size: 15px; margin: 0 0 2px; }
  .card .desc { color: var(--text-secondary); font-size: 12px; margin-bottom: 10px; }
  .empty { color: var(--text-muted); font-size: 13px; margin: 8px 0; }
  .legend { display: flex; gap: 14px; flex-wrap: wrap; font-size: 12px; color: var(--text-secondary); margin-bottom: 6px; }
  .legend span::before { content: ""; display: inline-block; width: 10px; height: 10px; border-radius: 3px;
    background: var(--c); margin-right: 6px; vertical-align: -1px; }
  svg text { font: 12px system-ui, -apple-system, "Segoe UI", sans-serif; fill: var(--text-secondary); }
  svg .val { fill: var(--text-primary); font-variant-numeric: tabular-nums; }
  svg .muted { fill: var(--text-muted); }
  table { width: 100%; border-collapse: collapse; font-size: 13px; }
  th, td { padding: 7px 8px; border-bottom: 1px solid var(--grid); text-align: right; font-variant-numeric: tabular-nums; }
  th:first-child, td:first-child { text-align: left; }
  th { color: var(--text-secondary); font-weight: 600; font-size: 12px; }
  tr.faded td, tr.unassigned td { color: var(--text-muted); }
  .pill { display: inline-block; min-width: 26px; padding: 1px 6px; border-radius: 6px; font-size: 11px;
    font-weight: 600; color: #fff; text-align: center; }
  .edited { color: var(--text-muted); font-size: 11px; }
  table.top td:nth-child(2), table.top th:nth-child(2) { text-align: left; }
  td.why, th.why { color: var(--text-secondary); font-size: 12px; text-align: left; }
  table.top a { color: inherit; }
  .method { color: var(--text-secondary); font-size: 13px; }
  .method table { width: auto; }
  .months .desc { margin-bottom: 12px; }
  table.matrix th, table.matrix td { text-align: center; }
  table.matrix th:first-child, table.matrix td:first-child { text-align: left; }
  table.matrix td.cell { color: var(--text-primary); }
  table.matrix td.cell .sub, table.matrix td.total .sub { color: var(--text-secondary); font-size: 11px; }
  table.matrix td.shaded .sub { color: var(--ink-on-shade); }
  table.matrix tr.faded td:not(:first-child) { opacity: 0.6; }
  table.matrix td.total { font-weight: 600; border-left: 1px solid var(--axis); }
  table.matrix td.none { color: var(--text-muted); }
  table.matrix tr.unassigned td, table.matrix tr.faded td:first-child { color: var(--text-muted); }
  details.month { border-top: 1px solid var(--grid); margin-top: 12px; padding-top: 8px; }
  details.month summary { cursor: pointer; font-weight: 600; font-size: 14px; padding: 4px 0; }
  details.month summary .meta { color: var(--text-secondary); font-weight: 400; }
  details.month h3 { font-size: 13px; margin: 14px 0 6px; color: var(--text-secondary); }
  .tiles.mini { margin: 12px 0; gap: 8px; }
  .tiles.mini .tile { padding: 8px 12px; }
  .tiles.mini .value { font-size: 20px; }
  @media print { .page { padding: 0; } .card, .tile { break-inside: avoid; } }
`;

function legendOfLevels(): string {
  return `<div class="legend">${LEVELS.map((l) => `<span style="--c:${LEVEL_COLOR[l]}">${l}</span>`).join("")}</div>`;
}

function membersTable(
  members: ReportMember[],
  unassigned: ReportData["unassigned"],
  emptyText = "No members to show in this period.",
): string {
  if (members.length === 0 && !unassigned) {
    return empty(emptyText);
  }
  const head = `<tr><th>Member</th><th>Points</th><th>Share</th><th>Delivered</th><th>Coverage</th>${LEVELS.map((l) => `<th>${l}</th>`).join("")}<th>Avg / ticket</th><th>Active</th></tr>`;
  const rows = members.map(
    (m) =>
      `<tr${m.faded ? ' class="faded"' : ""}><td>${esc(m.name)}</td><td>${fmt(m.points)}</td><td>${m.sharePercent}%</td><td>${m.delivered}</td><td>${m.scored}/${m.delivered}</td>${LEVELS.map((l) => `<td>${m.levels[l]}</td>`).join("")}<td>${m.avgPoints === null ? "-" : fmt(m.avgPoints)}</td><td>${m.active}</td></tr>`,
  );
  if (unassigned) {
    rows.push(
      `<tr class="unassigned"><td>Unassigned</td><td>-</td><td>-</td><td>${unassigned.delivered}</td><td>${unassigned.scored}/${unassigned.delivered}</td><td colspan="${LEVELS.length + 2}"></td></tr>`,
    );
  }
  return `<table>${head}${rows.join("")}</table>`;
}

function hardestTable(
  hardest: HardestTicket[],
  emptyText = "No scored tickets in this period.",
): string {
  if (hardest.length === 0) return empty(emptyText);
  const head = `<tr><th>Ticket</th><th>Member</th><th>Level</th><th>Points</th><th class="why">Main reason</th></tr>`;
  const rows = hardest.map((h) => {
    const title = h.url
      ? `<a href="${esc(h.url)}" target="_blank" rel="noopener noreferrer">${esc(h.title)}</a>`
      : esc(h.title);
    return `<tr><td>${title}</td><td>${esc(h.members)}</td><td><span class="pill" style="background:${LEVEL_COLOR[h.level]}">${h.level}</span>${h.edited ? ' <span class="edited">edited</span>' : ""}</td><td>${fmt(h.points)}</td><td class="why">${esc(h.reason)}</td></tr>`;
  });
  return `<table class="top">${head}${rows.join("")}</table>`;
}

/** Points with the delivered count small and muted next to them. */
function matrixCell(
  cell: MatrixCell,
  unassigned: boolean,
  title: string,
  total: boolean,
): string {
  const klass = total ? "total" : cell.shade === null ? "cell" : "cell shaded";
  if (cell.delivered === 0 && cell.points === 0) {
    return `<td class="none${total ? " total" : ""}" title="${esc(title)}">-</td>`;
  }
  const style =
    cell.shade === null ? "" : ` style="background:${CELL_RAMP[cell.shade]}"`;
  const body = unassigned
    ? String(cell.delivered)
    : `${fmt(cell.points)} <span class="sub">&middot; ${cell.delivered}</span>`;
  return `<td class="${klass}"${style} title="${esc(title)}">${body}</td>`;
}

function matrixTable(monthly: ReportMonthly, periodLabel: string): string {
  if (monthly.rows.length === 0) {
    return empty("No delivered tickets in this period.");
  }
  const head = `<tr><th>Member</th>${monthly.months.map((m) => `<th>${esc(m.label)}</th>`).join("")}<th>Total</th></tr>`;
  const text = (c: MatrixCell, unassigned: boolean) =>
    `${unassigned ? "" : `${fmt(c.points)} points, `}${c.delivered} delivered, ${c.scored} scored`;
  const rows = monthly.rows.map((row) => {
    const cells = row.cells.map((c, i) =>
      matrixCell(
        c,
        row.unassigned,
        `${row.name}, ${monthly.months[i].title}: ${text(c, row.unassigned)}`,
        false,
      ),
    );
    cells.push(
      matrixCell(
        row.total,
        row.unassigned,
        `${row.name}, ${periodLabel}: ${text(row.total, row.unassigned)}`,
        true,
      ),
    );
    const rowClass = row.unassigned ? "unassigned" : row.faded ? "faded" : "";
    return `<tr${rowClass ? ` class="${rowClass}"` : ""}><td>${esc(row.name)}</td>${cells.join("")}</tr>`;
  });
  return `<table class="matrix">${head}${rows.join("")}</table>`;
}

function monthBlock(month: ReportMonth): string {
  const tiles = month.tiles
    .map(
      (t) =>
        `<div class="tile"><div class="label">${esc(t.label)}</div><div class="value">${esc(t.value)}</div>${t.sub ? `<div class="sub">${esc(t.sub)}</div>` : ""}</div>`,
    )
    .join("");
  const note = month.note
    ? `<div class="notes"><div class="note"><b>${esc(month.note.headline)}</b> ${esc(month.note.text)}</div></div>`
    : "";
  return `<details class="month">
      <summary>${esc(month.title)} <span class="meta">&middot; ${month.delivered} delivered &middot; ${fmt(month.points)} points</span></summary>
      ${note}
      <div class="tiles mini">${tiles}</div>
      <h3>Member table</h3>
      ${membersTable(month.members, month.unassigned, "No members to show in this month.")}
      <h3>Hardest tickets</h3>
      ${hardestTable(month.hardest, "No scored tickets in this month.")}
    </details>`;
}

function monthlySection(data: ReportData): string {
  const monthly = data.monthly;
  if (!monthly || monthly.months.length === 0) return "";
  const blocks =
    monthly.rows.length === 0
      ? ""
      : monthly.months.map(monthBlock).join("\n    ");
  return `<section class="card months">
    <h2>Month by month</h2>
    ${blocks === "" ? "" : '<div class="desc">Click a month to see its details. Expand the months you want before printing.</div>'}
    ${matrixTable(monthly, data.periodLabel)}
    ${blocks}
  </section>
`;
}

function methodSection(data: ReportData): string {
  const sums = ["0-1", "2-3", "4-5", "6-7", "8-10"];
  const delivered =
    data.statuses.delivered.length > 0
      ? data.statuses.delivered.join(", ")
      : "none in this period";
  const excluded =
    data.statuses.excluded.length > 0
      ? ` Excluded statuses: ${data.statuses.excluded.join(", ")}.`
      : "";
  const members =
    data.excludedMembers.length > 0
      ? ` Excluded from this board: ${data.excludedMembers.join(", ")}.`
      : "";
  return `<section class="card method">
    <h2>How the score works</h2>
    <p>Each delivered ticket is judged by an AI on five criteria, 0 to 2 each: ambiguity, investigation, system scope, risk and coordination. The app sums them (0 to 10) and maps the sum to a level, which earns points. Difficulty is about the problem, not the size of the code change. Due dates are not used.</p>
    <table>
      <tr><th>Sum</th>${sums.map((s) => `<th>${s}</th>`).join("")}</tr>
      <tr><td>Level</td>${LEVELS.map((l) => `<td>${l}</td>`).join("")}</tr>
      <tr><td>Points</td>${LEVELS.map((l) => `<td>${fmt(data.points[l])}</td>`).join("")}</tr>
    </table>
    <p>Delivered statuses: ${esc(delivered)}.${esc(excluded)} Subtasks count; a parent with subtasks does not. Active tickets are shown for context and never scored.${esc(members)}</p>
    <p><b>Read this as a conversation starter, not a performance rating.</b> Work outside ClickUp, mentoring and unscored tickets are not in these numbers.</p>
  </section>`;
}

export function renderReportHtml(data: ReportData): string {
  const meta = [
    `Workspace <b>${esc(data.workspaceName)}</b>`,
    `Period <b>${esc(data.periodLabel)}</b>`,
    `Generated ${esc(data.generatedLabel)}`,
    ...(data.scoredWith ? [`Scored with ${esc(data.scoredWith)}`] : []),
    "Monoptah",
  ].join(" &middot; ");
  const noteRows = [
    ...(data.comparisonNote
      ? [`<div class="note">${esc(data.comparisonNote)}</div>`]
      : []),
    ...data.notes.map(
      (n) => `<div class="note"><b>${esc(n.headline)}</b> ${esc(n.text)}</div>`,
    ),
  ];
  const notes =
    noteRows.length > 0 ? `<div class="notes">${noteRows.join("")}</div>` : "";
  const tiles = data.tiles
    .map(
      (t) =>
        `<div class="tile"><div class="label">${esc(t.label)}</div><div class="value">${esc(t.value)}</div>${t.sub ? `<div class="sub">${esc(t.sub)}</div>` : ""}</div>`,
    )
    .join("");
  const unit = data.trend.unit;

  return `<!doctype html>
<html lang="en">
<head>
<meta charset="utf-8">
<title>${esc(`Ticket Contributors report - ${data.workspaceName} - ${data.periodLabel}`)}</title>
<meta name="viewport" content="width=device-width, initial-scale=1">
<style>${CSS}</style>
</head>
<body class="viz-root">
<div class="page">
  <header>
    <h1>Ticket Contributors report</h1>
    <div class="meta">${meta}</div>
  </header>
  ${notes}
  <section class="tiles" aria-label="Summary">${tiles}</section>

  <div class="grid2">
    <section class="card">
      <h2>Points by member</h2>
      <div class="desc">Sum of ticket points, split equally between assignees.</div>
      ${pointsChart(data.members)}
    </section>
    <section class="card">
      <h2>Difficulty mix by member</h2>
      <div class="desc">Scored tickets by level, light = small, dark = hard.</div>
      ${legendOfLevels()}
      ${levelsChart(data.members)}
    </section>
  </div>

  <div class="grid2">
    <section class="card">
      <h2>Team points per ${unit}</h2>
      <div class="desc">By delivery ${unit}, scored tickets only.</div>
      ${trendChart(data.trend)}
    </section>
    <section class="card">
      <h2>Delivered and active work</h2>
      <div class="desc">Delivered tickets in the period next to tickets still in a working status.</div>
      <div class="legend"><span style="--c:${COLOR.series1}">Delivered</span><span style="--c:${COLOR.series2}">Active</span></div>
      ${loadChart(data.load)}
    </section>
  </div>

  ${monthlySection(data)}
  <section class="card">
    <h2>Member table</h2>
    <div class="desc">Every number behind the charts. Coverage is scored tickets out of delivered tickets. A ticket with several assignees counts for each of them; unassigned tickets have their own row.</div>
    ${membersTable(data.members, data.unassigned)}
  </section>

  <section class="card">
    <h2>Hardest tickets this period</h2>
    <div class="desc">Top ${HARDEST_LIMIT} by level, with the AI's main reason. Overridden levels are marked "edited".</div>
    ${hardestTable(data.hardest)}
  </section>

  ${methodSection(data)}
</div>
</body>
</html>
`;
}
