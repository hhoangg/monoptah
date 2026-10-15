import type { Resolver } from "./identity";
import type { Leaderboard, LeaderboardRow } from "./leaderboard";
import { SCORING, attributeCommits } from "./scoring";
import { isInPeriod } from "./period";
import type { Period } from "./period";
import type { PrScore, PeriodScores } from "./scoring";
import type { Stats } from "./stats";
import type { ContributorsConfig, RawData } from "./types";

export type SnapshotScope =
  | { kind: "team" }
  | { kind: "person"; personId: string }
  | { kind: "me"; personId: string };

export type SnapshotPerson = {
  personId: string;
  name: string;
  rank: number;
  movement: number | null;
  score: number;
  /** Weighted contributions to `score` (quality and weights applied); they add up to it. */
  parts: {
    delivery: number;
    care: number;
    review: number;
    consistency: number;
    breadth: number;
  };
  /** Unweighted subtotals; `delivery` is before quality, `deliveryFinal` after. */
  breakdown: {
    delivery: number;
    deliveryFinal: number;
    quality: number;
    survival: number | null;
    care: number;
    review: number;
    consistency: number;
    breadth: number;
  };
  activeDays: number;
  streak: number;
  mergedPrs: number;
  mainCommits: number;
  reviewedPrs: number;
  aiAssistedPct: number;
  modules: string[];
  badges: string[];
  identities: string[];
};

export type SnapshotPr = PrScore & { personId: string; personName: string };

export type SnapshotCommit = {
  hash: string;
  author: string;
  person: string | null;
  date: string;
  files: number;
};

export type Snapshot = {
  generatedAt: string;
  project: string;
  branch: string;
  period: {
    kind: string;
    id: string;
    label: string;
    start: string;
    end: string;
  };
  scope: SnapshotScope;
  ghAvailable: boolean;
  formula: {
    constants: typeof SCORING;
    weights: ContributorsConfig["weights"];
    ignore: string[];
    description: string[];
  };
  people: SnapshotPerson[];
  focus?: SnapshotPerson | null;
  stats?: {
    kpis: Stats["kpis"];
    rows: Omit<Stats["rows"][number], "color">[];
  };
  prs?: SnapshotPr[];
  commits?: SnapshotCommit[];
  notes: string[];
};

const MAX_COMMITS = 2000;

const round2 = (n: number) => Math.round(n * 100) / 100;
const roundOrNull = (n: number | null) => (n === null ? null : round2(n));

export const snapshotFileName = (period: Period) =>
  `contributors-${period.id}.json`;

function formulaDescription(): string[] {
  const s = SCORING;
  return [
    "Score = Delivery * Quality + Care + Review + Consistency + Breadth. Each dimension is multiplied by its weight before it is added.",
    `Delivery: every merged PR earns ${s.PR_BASE} + ${s.SIZE_K} * log2(1 + effective lines) + iteration, capped at ${s.PR_CAP}. Effective lines = added + 0.5 * deleted of the final diff, after the ignore globs (lockfiles, generated files, snapshots, vendored code). Squash, rebase and merge commits are scored alike because the PR is the unit.`,
    `Iteration: ${s.ITER_K} * log2(1 + commits in the PR), capped at ${s.ITER_CAP}, so work hidden by a squash still counts. Closed PRs earn no Delivery.`,
    `Quality: a multiplier on Delivery only, ${s.QUALITY_BASE} + ${s.QUALITY_SLOPE} * survival, clamped to ${s.QUALITY_MIN}-${s.QUALITY_MAX}. Survival is the share of the lines a person wrote that are still alive at HEAD (measured for PRs at least 30 days old); no sample means 1.0. A reverted PR loses its Delivery and Care.`,
    `Care: a bonus on a PR's points, +${s.CARE_TEST * 100}% when it touches tests, +${s.CARE_DOCS * 100}% docs, +${s.CARE_NET_NEGATIVE * 100}% when it deletes more than it adds, +${s.CARE_FIX * 100}% for a fix or a closed bug issue.`,
    `Review (needs the GitHub CLI): ${s.REVIEW_PR} points per PR reviewed for someone else, plus ${s.REVIEW_COMMENT} per substantive comment, at most ${s.REVIEW_COMMENT_CAP} per PR. Reviewing your own PR earns nothing.`,
    `Consistency: ${s.ACTIVE_DAY} points per active day (days with a commit on the mainline or on a PR branch) plus ${s.STREAK_K} per day of the longest streak, capped at ${s.STREAK_CAP} days. Weekends are skipped for the streak: they neither break it nor extend it, but a weekend day with a commit is still an active day.`,
    `Breadth: ${s.MODULE_PTS} points for each module (first two folders of a path) with at least ${s.MODULE_MIN_EFF} effective lines of the person's own PRs, at most ${s.MODULE_CAP} modules.`,
    "Co-authors: with human Co-authored-by trailers the PR author gets 60% and the others share 40%. Agents never take a share.",
    "AI agents: work co-authored by an agent counts fully for the human behind it. Agent-authored work goes to the PR author, then the committer, then the identity mapping. AI-assisted % is informational and never changes a score.",
    "Periods are the last 7 days or a calendar month, quarter or year in local time, and days are bucketed by committer date. Rank movement is the previous rank minus the current rank.",
  ];
}

function toPerson(row: LeaderboardRow, resolver: Resolver): SnapshotPerson {
  // Each part is rounded first and the score is their sum, so the numbers the
  // agent reads always add up.
  const parts = {
    delivery: round2(row.parts.delivery),
    care: round2(row.parts.care),
    review: round2(row.parts.review),
    consistency: round2(row.parts.consistency),
    breadth: round2(row.parts.breadth),
  };
  return {
    personId: row.personId,
    name: row.name,
    rank: row.rank,
    movement: row.movement,
    score: round2(Object.values(parts).reduce((sum, part) => sum + part, 0)),
    parts,
    breakdown: {
      delivery: round2(row.delivery),
      deliveryFinal: round2(row.deliveryFinal),
      quality: round2(row.quality),
      survival: roundOrNull(row.survival),
      care: round2(row.care),
      review: round2(row.review),
      consistency: round2(row.consistency),
      breadth: round2(row.breadth),
    },
    activeDays: row.activeDays,
    streak: row.streak,
    mergedPrs: row.mergedPrs,
    mainCommits: row.mainCommits,
    reviewedPrs: row.reviewedPrs,
    aiAssistedPct: row.aiAssistedPct,
    modules: row.modules,
    badges: row.badges.map((b) => b.label),
    identities: resolver.people.find((p) => p.id === row.personId)
      ?.identities ?? [row.personId],
  };
}

function toPr(row: LeaderboardRow, pr: PrScore): SnapshotPr {
  return {
    ...pr,
    personId: row.personId,
    personName: row.name,
    points: round2(pr.points),
    care: round2(pr.care),
    deliveryShare: round2(pr.deliveryShare),
    iteration: round2(pr.iteration),
    size: round2(pr.size),
  };
}

function buildNotes(
  scores: PeriodScores,
  raw: RawData | undefined,
  ghAvailable: boolean,
  commitsCapped: boolean,
  weights: ContributorsConfig["weights"],
): string[] {
  const notes = ["Scores are for motivation, not for performance reviews."];
  const custom = Object.entries(weights).filter(([, w]) => w !== 1);
  if (custom.length > 0) {
    notes.push(
      `Custom weights are in use (${custom.map(([k, w]) => `${k} ${w}`).join(", ")}). "parts" already include them; "breakdown" does not.`,
    );
  }
  if (!ghAvailable) {
    const why = raw?.ghNote ? ` (${raw.ghNote})` : "";
    notes.push(
      `Review points are not counted because the GitHub CLI is unavailable${why}`,
    );
  }
  const { prs, commits } = scores.unattributed;
  const parts = [
    prs > 0 ? `${prs} ${prs === 1 ? "PR" : "PRs"}` : "",
    commits > 0 ? `${commits} ${commits === 1 ? "commit" : "commits"}` : "",
  ].filter(Boolean);
  if (parts.length > 0) {
    notes.push(
      `Work by AI agents that no human could be credited for is not scored: ${parts.join(" and ")}.`,
    );
  }
  if (raw?.truncated) {
    notes.push(
      "The collected history hit a safety cap, so older activity may be missing.",
    );
  }
  if (commitsCapped) {
    notes.push(`Only the ${MAX_COMMITS} most recent commits are listed.`);
  }
  return notes;
}

export function buildSnapshot(input: {
  project: string;
  branch: string;
  period: Period;
  scores: PeriodScores;
  leaderboard: Leaderboard;
  stats?: Stats;
  config: ContributorsConfig;
  scope: SnapshotScope;
  include: { perPr: boolean; rawCommits: boolean };
  raw?: RawData;
  ghAvailable: boolean;
  resolver: Resolver;
  now: Date;
}): Snapshot {
  const { period, scores, leaderboard, scope, include, raw, resolver } = input;
  const people = leaderboard.rows.map((row) => toPerson(row, resolver));
  const focusId = scope.kind === "team" ? null : scope.personId;

  let commits: SnapshotCommit[] | undefined;
  let commitsCapped = false;
  if (include.rawCommits && raw) {
    const inPeriod = attributeCommits({
      raw,
      config: input.config,
      resolver,
    })
      .filter(
        (c) =>
          isInPeriod(period, c.commit.committedAt) &&
          (focusId === null || c.personId === focusId),
      )
      .sort(
        (a, b) =>
          Date.parse(b.commit.committedAt) - Date.parse(a.commit.committedAt),
      );
    commitsCapped = inPeriod.length > MAX_COMMITS;
    commits = inPeriod.slice(0, MAX_COMMITS).map((c) => ({
      hash: c.commit.hash.slice(0, 12),
      author: `${c.commit.author.name} <${c.commit.author.email}>`,
      person: c.personId ? resolver.nameOf(c.personId) : null,
      date: c.commit.committedAt,
      files: c.files.length,
    }));
  }

  return {
    generatedAt: input.now.toISOString(),
    project: input.project,
    branch: input.branch,
    period: {
      kind: period.kind,
      id: period.id,
      label: period.label,
      start: period.start.toISOString(),
      end: period.end.toISOString(),
    },
    scope,
    ghAvailable: input.ghAvailable,
    formula: {
      constants: SCORING,
      weights: input.config.weights,
      ignore: input.config.ignore,
      description: formulaDescription(),
    },
    people,
    ...(focusId !== null
      ? { focus: people.find((p) => p.personId === focusId) ?? null }
      : {}),
    ...(input.stats
      ? {
          stats: {
            kpis: input.stats.kpis,
            rows: input.stats.rows.map(({ color: _color, ...row }) => row),
          },
        }
      : {}),
    ...(include.perPr
      ? {
          prs: leaderboard.rows
            .filter((row) => focusId === null || row.personId === focusId)
            .flatMap((row) => row.prs.map((pr) => toPr(row, pr))),
        }
      : {}),
    ...(commits ? { commits } : {}),
    notes: buildNotes(
      scores,
      raw,
      input.ghAvailable,
      commitsCapped,
      input.config.weights,
    ),
  };
}

const TEAM_QUESTIONS = [
  "Summarize this period: who drove what, and where are the risks?",
  "Which modules depend on a single person (bus factor)?",
  "Compare this period with the last 3 and explain the trend.",
  "Are any scores inflated by generated code or huge PRs?",
];

const ME_QUESTIONS = [
  "Why did my rank change since the previous period?",
  "What would raise my Quality factor?",
  "Which of my PRs had the most rework, and why?",
];

export function suggestedQuestions(
  scope: SnapshotScope,
  name?: string,
): string[] {
  if (scope.kind === "team") return [...TEAM_QUESTIONS];
  if (scope.kind === "me") return [...ME_QUESTIONS];
  const who = name ?? "this person";
  return [
    `Summarize ${who}'s contributions this period.`,
    `What are ${who}'s strengths across the 5 dimensions?`,
    `Show ${who}'s PRs with the most iteration and what slowed them down.`,
    `Is ${who}'s Delivery low because of review or mentoring work?`,
  ];
}

function scopeLine(scope: SnapshotScope, personName?: string): string {
  if (scope.kind === "team") return "the whole team";
  const who = personName ? `${personName} (${scope.personId})` : scope.personId;
  return scope.kind === "me"
    ? `one person asking about their own score: ${who}`
    : `one person: ${who}`;
}

const DEFAULT_CONTENTS = [
  "scores",
  "breakdowns",
  "identity mapping",
  "the scoring formula in use",
];

function listOf(items: string[]): string {
  if (items.length <= 1) return items.join("");
  return `${items.slice(0, -1).join(", ")} and ${items[items.length - 1]}`;
}

export function buildAgentPrompt(input: {
  question: string;
  scope: SnapshotScope;
  fileName: string;
  allowTools: boolean;
  projectName: string;
  periodLabel: string;
  personName?: string;
  /** What the file holds, as noun phrases; defaults to the full snapshot. */
  contains?: string[];
}): string {
  const tools = input.allowTools
    ? "You may run read-only git and gh commands inside this project to dig deeper. Do not change anything."
    : "Do not run commands; answer only from the attached snapshot.";
  return [
    `I am looking at the Contributors leaderboard of the project "${input.projectName}" for the period "${input.periodLabel}".`,
    `Scope of my question: ${scopeLine(input.scope, input.personName)}.`,
    `The attached file ${input.fileName} is the exact data I see: ${listOf(input.contains ?? DEFAULT_CONTENTS)}. Treat it as the source of truth for every number.`,
    tools,
    "",
    `Question: ${input.question}`,
  ].join("\n");
}
