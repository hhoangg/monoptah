import { identityKey, isAgent, isExcluded, isHuman } from "./identity";
import type { Resolver } from "./identity";
import { isIgnoredPath } from "./patterns";
import {
  isInPeriod,
  isWeekend,
  localDayKey,
  periodDayKeys,
  previousPeriod,
} from "./period";
import type { Period } from "./period";
import type {
  ContributorsConfig,
  GitIdentity,
  RawCommit,
  RawData,
  RawFile,
  RawPr,
} from "./types";

export const SCORING = {
  PR_BASE: 10,
  SIZE_K: 4,
  PR_CAP: 72,
  ITER_K: 3,
  ITER_CAP: 12,
  REVIEW_PR: 8,
  REVIEW_COMMENT: 1,
  REVIEW_COMMENT_CAP: 5,
  ACTIVE_DAY: 5,
  STREAK_K: 2,
  STREAK_CAP: 14,
  MODULE_PTS: 15,
  MODULE_MIN_EFF: 30,
  MODULE_CAP: 6,
  QUALITY_BASE: 0.8,
  QUALITY_SLOPE: 0.4,
  QUALITY_MIN: 0.8,
  QUALITY_MAX: 1.2,
  CARE_TEST: 0.2,
  CARE_DOCS: 0.1,
  CARE_NET_NEGATIVE: 0.15,
  CARE_FIX: 0.15,
  OWNER_SHARE: 0.6,
} as const;

export type PrScore = {
  id: string;
  title: string;
  /** Delivery points of the whole PR (0 when reverted); the person earns `points * deliveryShare`. */
  points: number;
  deliveryShare: number;
  /** Care bonus this person earned on the PR (already multiplied by the share). */
  care: number;
  iteration: number;
  size: number;
  added: number;
  deleted: number;
  commitCount: number;
  testFiles: boolean;
  docs: boolean;
  netNegative: boolean;
  fix: boolean;
  reverted: boolean;
  aiAssisted: boolean;
  mergedAt: string;
};

/**
 * `delivery`, `care`, `review`, `consistency`, `breadth` and `deliveryFinal`
 * are the unweighted subtotals (what the formula sidebar explains). `parts` is
 * the same five dimensions multiplied by `config.weights`, with quality already
 * applied to delivery, so `score` is exactly the sum of `parts`. Draw breakdown
 * bars from `parts`.
 */
export type PersonScore = {
  personId: string;
  name: string;
  score: number;
  delivery: number; // before quality
  deliveryFinal: number; // times quality
  quality: number;
  survival: number | null;
  care: number;
  review: number;
  consistency: number;
  breadth: number;
  activeDays: number;
  streak: number;
  mergedPrs: number;
  branchCommits: number; // sum of commitCount of owned merged PRs
  mainCommits: number;
  aiAssistedPct: number; // 0-100, informational
  fixPrs: number;
  netNegativePrs: number;
  reviewedPrs: number;
  parts: {
    delivery: number;
    care: number;
    review: number;
    consistency: number;
    breadth: number;
  };
  modules: string[];
  prs: PrScore[];
};

export type PeriodScores = {
  period: Period;
  people: PersonScore[];
  unattributed: { prs: number; commits: number };
  ghAvailable: boolean;
  /** People whose first activity in the data falls inside the period (empty when the data does not reach the previous period). */
  newcomers: string[];
};

export type CareFlags = {
  test: boolean;
  docs: boolean;
  netNegative: boolean;
  fix: boolean;
};

export type AttributedCommit = {
  commit: RawCommit;
  personId: string | null; // null: nobody to credit
  files: RawFile[]; // after the ignore globs
  aiAssisted: boolean;
};

const normalizePath = (path: string) =>
  path.replace(/\\/g, "/").replace(/^\.\//, "");

const TEST_RE =
  /(^|\/)(tests?|__tests__|spec)\/|\.(test|spec)\.[a-z]+$|_test\.(go|rs|py)$/i;
const DOCS_GLOBS = ["*.md", "*.mdx", "docs/**", "*.rst"];
const FIX_RE = /^fix(\(|:|!|\s)/i;

export const isTestPath = (path: string) => TEST_RE.test(normalizePath(path));
export const isDocsPath = (path: string) => isIgnoredPath(DOCS_GLOBS, path);

/** Files that survive the config's ignore globs. */
export function countedFiles(
  config: ContributorsConfig,
  files: RawFile[],
): RawFile[] {
  return files.filter((f) => !isIgnoredPath(config.ignore, f.path));
}

/** Effective lines: added + 0.5 * deleted. Pass files that are already ignore-filtered. */
export function effectiveLines(files: RawFile[]): number {
  let eff = 0;
  for (const f of files) eff += f.added + 0.5 * f.deleted;
  return eff;
}

export function prSize(files: RawFile[]): number {
  return Math.log2(1 + effectiveLines(files));
}

export function iterationPoints(commitCount: number): number {
  return Math.min(
    SCORING.ITER_CAP,
    SCORING.ITER_K * Math.log2(1 + Math.max(1, commitCount)),
  );
}

export function prPoints(files: RawFile[], commitCount: number): number {
  return Math.min(
    SCORING.PR_CAP,
    SCORING.PR_BASE +
      SCORING.SIZE_K * prSize(files) +
      iterationPoints(commitCount),
  );
}

/** `survival` is alive / written in 0..1, or null when nothing was sampled. */
export function qualityFactor(survival: number | null): number {
  if (survival === null) return 1;
  const raw = SCORING.QUALITY_BASE + SCORING.QUALITY_SLOPE * survival;
  return Math.min(SCORING.QUALITY_MAX, Math.max(SCORING.QUALITY_MIN, raw));
}

export function careRate(flags: CareFlags): number {
  return (
    (flags.test ? SCORING.CARE_TEST : 0) +
    (flags.docs ? SCORING.CARE_DOCS : 0) +
    (flags.netNegative ? SCORING.CARE_NET_NEGATIVE : 0) +
    (flags.fix ? SCORING.CARE_FIX : 0)
  );
}

/** First two segments of the directory part, `(root)` for a file at the repo root. */
export function moduleOf(path: string): string {
  const segments = normalizePath(path).split("/").filter(Boolean);
  const dir = segments.slice(0, -1);
  return dir.length === 0 ? "(root)" : dir.slice(0, 2).join("/");
}

/**
 * Longest run of consecutive active working days (Mon-Fri) in the period.
 * Weekend days are skipped entirely: they neither break nor extend the run,
 * though an active weekend day still counts as an active day elsewhere.
 * Holidays have no data source and are not modelled.
 */
export function streakOf(
  activeDayKeys: Iterable<string>,
  period: Period,
  now: Date,
): number {
  const active = new Set(activeDayKeys);
  let run = 0;
  let best = 0;
  for (const key of periodDayKeys(period, now)) {
    if (isWeekend(key)) continue;
    if (active.has(key)) {
      run += 1;
      best = Math.max(best, run);
    } else {
      run = 0;
    }
  }
  return best;
}

function forEachIdentity(raw: RawData, visit: (id: GitIdentity) => void) {
  for (const c of raw.commits) {
    visit(c.author);
    visit(c.committer);
    c.coAuthors.forEach(visit);
  }
  for (const pr of raw.prs) {
    visit(pr.author);
    if (pr.committer) visit(pr.committer);
    pr.landingCoAuthors.forEach(visit);
    for (const bc of pr.branchCommits) bc.authors.forEach(visit);
  }
  for (const r of raw.reviews) visit(r.reviewer);
}

/** Every distinct identity in the raw data, for `buildResolver({ observedIdentities })`. */
export function observedIdentities(raw: RawData): GitIdentity[] {
  const seen = new Map<string, GitIdentity>();
  forEachIdentity(raw, (id) => {
    const key = `${identityKey(id)}\u0000${id.login?.trim().toLowerCase() ?? ""}`;
    if (!seen.has(key)) seen.set(key, id);
  });
  return Array.from(seen.values());
}

type Ctx = {
  config: ContributorsConfig;
  resolver: Resolver;
  /** Every identity seen per identity key, including each name used with it. */
  variants: Map<string, GitIdentity[]>;
};

function makeCtx(
  raw: RawData,
  config: ContributorsConfig,
  resolver: Resolver,
): Ctx {
  const variants = new Map<string, GitIdentity[]>();
  forEachIdentity(raw, (id) => {
    const key = identityKey(id);
    const list = variants.get(key);
    if (!list) variants.set(key, [id]);
    else if (!list.some((v) => v.name === id.name && v.login === id.login)) {
      list.push(id);
    }
  });
  return { config, resolver, variants };
}

type Credit =
  | { status: "owned"; personId: string }
  | { status: "unattributed" } // an agent did the work and no human can be named
  | { status: "dropped" }; // a bot: not part of the data at all

const owned = (personId: string): Credit => ({ status: "owned", personId });

/** An identity string of a resolved person (email, `gh:<login>` or name) as a GitIdentity. */
function identityOfKey(key: string): GitIdentity {
  if (key.startsWith("gh:"))
    return { name: "", email: "", login: key.slice(3) };
  return key.includes("@")
    ? { name: "", email: key }
    : { name: key, email: "" };
}

/**
 * The person an agent or bot identity was mapped to, as opposed to being its
 * own person. Only a person with at least one human identity counts, so an
 * agent can never become a leaderboard entry through an automatic link.
 *
 * - A config person is the user's explicit choice: each listed key (email or
 *   `gh:<login>`) is judged by the key alone, never by a git display name, so
 *   `Devin <devin@company.com>` listed under a person is credited.
 * - Any other person (mailmap group, gh-login link) is judged by what was
 *   seen: a key is an agent only when every name used with it is one.
 */
function mappedPerson(ctx: Ctx, id: GitIdentity): string | null {
  const personId = ctx.resolver.personOf(id);
  if (personId === identityKey(id)) return null;
  const configured = ctx.config.people.some((p) => p.id === personId);
  const person = ctx.resolver.people.find((p) => p.id === personId);
  const hasHuman = (person?.identities ?? [personId]).some((key) => {
    const bare = identityOfKey(key);
    if (configured) return isHuman(ctx.config, bare);
    const seen = ctx.variants.get(key.toLowerCase());
    return (seen ?? [bare]).some((v) => isHuman(ctx.config, v));
  });
  return hasHuman ? personId : null;
}

/** Owner 60% and human co-authors 40% split equally; the owner alone gets 100%. */
function sharesOf(ctx: Ctx, pr: RawPr, owner: string): [string, number][] {
  const coAuthors = Array.from(
    new Set(
      pr.landingCoAuthors
        .filter((id) => isHuman(ctx.config, id))
        .map((id) => ctx.resolver.personOf(id)),
    ),
  ).filter((id) => id !== owner);
  if (coAuthors.length === 0) return [[owner, 1]];
  return [
    [owner, SCORING.OWNER_SHARE],
    ...coAuthors.map((id): [string, number] => [
      id,
      (1 - SCORING.OWNER_SHARE) / coAuthors.length,
    ]),
  ];
}

/** Author, else human committer, else an identity the user mapped to a person. */
function creditFallback(
  ctx: Ctx,
  author: GitIdentity,
  committer: GitIdentity | undefined,
): Credit {
  const { config, resolver } = ctx;
  if (committer && isHuman(config, committer)) {
    return owned(resolver.personOf(committer));
  }
  const mapped =
    mappedPerson(ctx, author) ??
    (committer ? mappedPerson(ctx, committer) : null);
  if (mapped) return owned(mapped);
  const agentInvolved =
    isAgent(config, author) || (!!committer && isAgent(config, committer));
  return agentInvolved ? { status: "unattributed" } : { status: "dropped" };
}

function creditPr(ctx: Ctx, pr: RawPr): Credit {
  if (isHuman(ctx.config, pr.author))
    return owned(ctx.resolver.personOf(pr.author));
  return creditFallback(ctx, pr.author, pr.committer);
}

function creditPrs(ctx: Ctx, prs: RawPr[]): Map<string, Credit> {
  return new Map(prs.map((pr) => [pr.id, creditPr(ctx, pr)]));
}

function creditCommit(
  ctx: Ctx,
  commit: RawCommit,
  prCredits: Map<string, Credit>,
): Credit {
  const { config, resolver } = ctx;
  if (isExcluded(config, commit.author)) return { status: "dropped" };
  if (isHuman(config, commit.author))
    return owned(resolver.personOf(commit.author));
  const viaPr = commit.prId ? prCredits.get(commit.prId) : undefined;
  if (viaPr?.status === "owned") return viaPr;
  return creditFallback(ctx, commit.author, commit.committer);
}

const hasAgent = (config: ContributorsConfig, ids: GitIdentity[]) =>
  ids.some((id) => isAgent(config, id));

function attributeWith(
  ctx: Ctx,
  commits: RawCommit[],
  prCredits: Map<string, Credit>,
): AttributedCommit[] {
  const out: AttributedCommit[] = [];
  for (const commit of commits) {
    const credit = creditCommit(ctx, commit, prCredits);
    if (credit.status === "dropped") continue;
    out.push({
      commit,
      personId: credit.status === "owned" ? credit.personId : null,
      files: countedFiles(ctx.config, commit.files),
      aiAssisted: hasAgent(ctx.config, [commit.author, ...commit.coAuthors]),
    });
  }
  return out;
}

/**
 * Who each non-bot commit counts for. An agent-authored commit goes to the
 * owner of its PR, else the committer, else a mapped identity, else nobody.
 */
export function attributeCommits(input: {
  raw: RawData;
  config: ContributorsConfig;
  resolver: Resolver;
}): AttributedCommit[] {
  const ctx = makeCtx(input.raw, input.config, input.resolver);
  return attributeWith(ctx, input.raw.commits, creditPrs(ctx, input.raw.prs));
}

/** Who each non-bot PR counts for; `null` for an agent PR no human can be named for. */
export function attributePrs(input: {
  raw: RawData;
  config: ContributorsConfig;
  resolver: Resolver;
}): { pr: RawPr; personId: string | null }[] {
  const ctx = makeCtx(input.raw, input.config, input.resolver);
  const out: { pr: RawPr; personId: string | null }[] = [];
  for (const pr of input.raw.prs) {
    const credit = creditPr(ctx, pr);
    if (credit.status === "dropped") continue;
    out.push({
      pr,
      personId: credit.status === "owned" ? credit.personId : null,
    });
  }
  return out;
}

type Acc = {
  personId: string;
  delivery: number;
  care: number;
  review: number;
  reviewedPrs: number;
  branchCommits: number;
  mainCommits: number;
  days: Set<string>;
  prs: PrScore[];
  prIds: Set<string>;
  moduleEff: Map<string, number>;
  written: number;
  alive: number;
  units: number;
  aiUnits: number;
};

const newAcc = (personId: string): Acc => ({
  personId,
  delivery: 0,
  care: 0,
  review: 0,
  reviewedPrs: 0,
  branchCommits: 0,
  mainCommits: 0,
  days: new Set(),
  prs: [],
  prIds: new Set(),
  moduleEff: new Map(),
  written: 0,
  alive: 0,
  units: 0,
  aiUnits: 0,
});

export function scorePeriod(input: {
  raw: RawData;
  config: ContributorsConfig;
  resolver: Resolver;
  period: Period;
  /** Today; the streak does not look at days after it. */
  now: Date;
}): PeriodScores {
  const { raw, config, resolver, period, now } = input;
  const ctx = makeCtx(raw, config, resolver);
  const prCredits = creditPrs(ctx, raw.prs);
  const accs = new Map<string, Acc>();
  const firstActivity = new Map<string, number>();
  const unattributed = { prs: 0, commits: 0 };

  const acc = (personId: string) => {
    let a = accs.get(personId);
    if (!a) {
      a = newAcc(personId);
      accs.set(personId, a);
    }
    return a;
  };
  const note = (personId: string, iso: string) => {
    const t = Date.parse(iso);
    const current = firstActivity.get(personId);
    if (!Number.isNaN(t) && (current === undefined || t < current)) {
      firstActivity.set(personId, t);
    }
  };
  const markDay = (personId: string, iso: string) => {
    if (isInPeriod(period, iso)) acc(personId).days.add(localDayKey(iso));
  };

  const commitsByPr = new Map<string, RawCommit[]>();
  for (const c of raw.commits) {
    if (!c.prId) continue;
    const list = commitsByPr.get(c.prId);
    if (list) list.push(c);
    else commitsByPr.set(c.prId, [c]);
  }
  const survivalByPr = new Map<string, { written: number; alive: number }>();
  for (const s of raw.survival) {
    const cur = survivalByPr.get(s.prId) ?? { written: 0, alive: 0 };
    survivalByPr.set(s.prId, {
      written: cur.written + s.written,
      alive: cur.alive + s.alive,
    });
  }
  const reverted = new Set(raw.revertedPrIds);

  for (const pr of raw.prs) {
    const credit = prCredits.get(pr.id);
    const owner = credit?.status === "owned" ? credit.personId : null;
    const when = pr.state === "merged" ? pr.mergedAt : pr.closedAt;
    const inPeriod =
      when !== undefined && isInPeriod(period, when) ? when : null;

    if (owner && when !== undefined) note(owner, when);
    for (const bc of pr.branchCommits) {
      const people = new Set<string>(owner ? [owner] : []);
      for (const author of bc.authors) {
        if (isHuman(config, author)) people.add(resolver.personOf(author));
      }
      for (const personId of people) {
        note(personId, bc.committedAt);
        // markDay keeps only days inside the period, so a PR that closes after
        // the period still credits its days here.
        markDay(personId, bc.committedAt);
      }
    }
    if (
      owner &&
      inPeriod &&
      pr.state === "closed" &&
      pr.branchCommits.length === 0
    ) {
      markDay(owner, inPeriod);
    }

    if (pr.state !== "merged" || !inPeriod) continue;
    if (credit?.status === "unattributed") unattributed.prs += 1;
    if (!owner) continue;

    const files = countedFiles(config, pr.files);
    const added = files.reduce((sum, f) => sum + f.added, 0);
    const deleted = files.reduce((sum, f) => sum + f.deleted, 0);
    const flags: CareFlags = {
      test: files.some((f) => isTestPath(f.path)),
      docs: files.some((f) => isDocsPath(f.path)),
      netNegative: deleted > added,
      fix: FIX_RE.test(pr.title) || pr.closesBug,
    };
    const isReverted = reverted.has(pr.id);
    const points = isReverted ? 0 : prPoints(files, pr.commitCount);
    const rate = careRate(flags);
    const aiAssisted =
      hasAgent(config, [pr.author, ...pr.landingCoAuthors]) ||
      pr.branchCommits.some((bc) => hasAgent(config, bc.authors)) ||
      (commitsByPr.get(pr.id) ?? []).some((c) =>
        hasAgent(config, [c.author, ...c.coAuthors]),
      );

    const shares = sharesOf(ctx, pr, owner);

    // A reverted PR's dead lines must not also lower the quality of the rest.
    const sample = isReverted ? undefined : survivalByPr.get(pr.id);
    for (const [personId, share] of shares) {
      const a = acc(personId);
      const care = share * points * rate;
      a.delivery += share * points;
      a.care += care;
      a.prIds.add(pr.id);
      a.units += 1;
      if (aiAssisted) a.aiUnits += 1;
      if (sample && sample.written > 0) {
        a.written += sample.written;
        a.alive += sample.alive;
      }
      a.prs.push({
        id: pr.id,
        title: pr.title,
        points,
        deliveryShare: share,
        care,
        iteration: iterationPoints(pr.commitCount),
        size: prSize(files),
        added,
        deleted,
        commitCount: pr.commitCount,
        testFiles: flags.test,
        docs: flags.docs,
        netNegative: flags.netNegative,
        fix: flags.fix,
        reverted: isReverted,
        aiAssisted,
        mergedAt: inPeriod,
      });
      note(personId, inPeriod);
    }

    const a = acc(owner);
    a.branchCommits += pr.commitCount;
    for (const f of files) {
      const module = moduleOf(f.path);
      a.moduleEff.set(
        module,
        (a.moduleEff.get(module) ?? 0) + f.added + 0.5 * f.deleted,
      );
    }
  }

  for (const { commit, personId, aiAssisted } of attributeWith(
    ctx,
    raw.commits,
    prCredits,
  )) {
    if (personId) note(personId, commit.committedAt);
    if (!isInPeriod(period, commit.committedAt)) continue;
    if (!personId) {
      unattributed.commits += 1;
      continue;
    }
    const a = acc(personId);
    a.mainCommits += 1;
    a.days.add(localDayKey(commit.committedAt));
    if (!commit.prId || !a.prIds.has(commit.prId)) {
      a.units += 1;
      if (aiAssisted) a.aiUnits += 1;
    }
  }

  if (raw.ghAvailable) {
    const prById = new Map(raw.prs.map((pr) => [pr.id, pr]));
    const pairs = new Map<
      string,
      { reviewerId: string; prId: string; comments: number }
    >();
    for (const review of raw.reviews) {
      // Reviews of PRs missing from raw.prs (e.g. still open) are skipped: the
      // author is unknown, so a self review could not be ruled out.
      const pr = prById.get(review.prId);
      if (!pr || !isHuman(config, review.reviewer)) continue;
      const reviewerId = resolver.personOf(review.reviewer);
      const credit = prCredits.get(pr.id);
      const ownerId = credit?.status === "owned" ? credit.personId : null;
      const sharing =
        ownerId !== null &&
        sharesOf(ctx, pr, ownerId).some(([id]) => id === reviewerId);
      if (sharing || reviewerId === resolver.personOf(pr.author)) continue;
      note(reviewerId, review.submittedAt);
      if (!isInPeriod(period, review.submittedAt)) continue;
      const key = `${reviewerId}\u0000${pr.id}`;
      const pair = pairs.get(key) ?? { reviewerId, prId: pr.id, comments: 0 };
      pair.comments += review.comments;
      pairs.set(key, pair);
    }
    for (const { reviewerId, comments } of pairs.values()) {
      const a = acc(reviewerId);
      a.review +=
        SCORING.REVIEW_PR +
        Math.min(SCORING.REVIEW_COMMENT_CAP, SCORING.REVIEW_COMMENT * comments);
      a.reviewedPrs += 1;
    }
  }

  const people = Array.from(accs.values()).map((a) =>
    finishPerson(a, config, resolver, period, now),
  );
  people.sort(
    (a, b) =>
      b.score - a.score ||
      a.name.localeCompare(b.name) ||
      a.personId.localeCompare(b.personId),
  );

  const dataReachesPreviousPeriod =
    Date.parse(raw.since) <= previousPeriod(period).start.getTime();
  const newcomers = dataReachesPreviousPeriod
    ? people
        .map((p) => p.personId)
        .filter((id) => (firstActivity.get(id) ?? 0) >= period.start.getTime())
        .sort()
    : [];

  return {
    period,
    people,
    unattributed,
    ghAvailable: raw.ghAvailable,
    newcomers,
  };
}

function finishPerson(
  a: Acc,
  config: ContributorsConfig,
  resolver: Resolver,
  period: Period,
  now: Date,
): PersonScore {
  const survival = a.written > 0 ? a.alive / a.written : null;
  const quality = qualityFactor(survival);
  const modules = Array.from(a.moduleEff)
    .filter(([, eff]) => eff >= SCORING.MODULE_MIN_EFF)
    .sort((x, y) => y[1] - x[1] || x[0].localeCompare(y[0]))
    .slice(0, SCORING.MODULE_CAP)
    .map(([module]) => module);
  const streak = streakOf(a.days, period, now);
  const breadth = SCORING.MODULE_PTS * modules.length;
  const consistency =
    SCORING.ACTIVE_DAY * a.days.size +
    SCORING.STREAK_K * Math.min(streak, SCORING.STREAK_CAP);
  const w = config.weights;
  const parts = {
    delivery: a.delivery * quality * w.delivery,
    care: a.care * w.care,
    review: a.review * w.review,
    consistency: consistency * w.consistency,
    breadth: breadth * w.breadth,
  };
  const live = a.prs.filter((p) => !p.reverted);
  return {
    personId: a.personId,
    name: resolver.nameOf(a.personId),
    score:
      parts.delivery +
      parts.care +
      parts.review +
      parts.consistency +
      parts.breadth,
    delivery: a.delivery,
    deliveryFinal: a.delivery * quality,
    quality,
    survival,
    care: a.care,
    review: a.review,
    consistency,
    breadth,
    activeDays: a.days.size,
    streak,
    mergedPrs: a.prs.length,
    branchCommits: a.branchCommits,
    mainCommits: a.mainCommits,
    aiAssistedPct: a.units === 0 ? 0 : Math.round((a.aiUnits / a.units) * 100),
    fixPrs: live.filter((p) => p.fix).length,
    netNegativePrs: live.filter((p) => p.netNegative).length,
    reviewedPrs: a.reviewedPrs,
    parts,
    modules,
    prs: a.prs,
  };
}
