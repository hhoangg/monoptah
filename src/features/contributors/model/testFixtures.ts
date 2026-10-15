// Builders shared by the model tests. Not imported by app code.
import { DEFAULT_CONFIG } from "./config";
import { buildResolver } from "./identity";
import { observedIdentities } from "./scoring";
import { currentPeriod } from "./period";
import type { Period } from "./period";
import type {
  ContributorsConfig,
  GitIdentity,
  RawCommit,
  RawData,
  RawFile,
  RawPr,
  RawReview,
} from "./types";

/** ISO timestamp of a local day in October 2026 (Oct 1 2026 is a Thursday). */
export const oct = (day: number, hour = 12) =>
  new Date(2026, 9, day, hour).toISOString();

/** October 2026 as a period. */
export const PERIOD: Period = currentPeriod("month", new Date(2026, 9, 15));

/** The last day of `PERIOD`, so every day of it counts as elapsed. */
export const PERIOD_NOW = new Date(2026, 9, 31, 12);

export const ident = (name: string, email: string, login?: string) =>
  login ? { name, email, login } : { name, email };

export const ANA = ident("Ana", "ana@example.com", "ana");
export const BOB = ident("Bob", "bob@example.com", "bob");
export const CAT = ident("Cat", "cat@example.com", "cat");
export const CLAUDE = ident("Claude", "noreply@anthropic.com");
export const DEPENDABOT = ident(
  "dependabot[bot]",
  "49699333+dependabot[bot]@users.noreply.github.com",
  "dependabot[bot]",
);
export const GITHUB = ident("GitHub", "noreply@github.com");

export const file = (path: string, added: number, deleted = 0): RawFile => ({
  path,
  added,
  deleted,
});

export function makeCommit(
  hash: string,
  author: GitIdentity,
  committedAt: string,
  extra: Partial<RawCommit> = {},
): RawCommit {
  return {
    hash,
    subject: `commit ${hash}`,
    author,
    committer: author,
    authoredAt: committedAt,
    committedAt,
    coAuthors: [],
    files: [],
    ...extra,
  };
}

export function makePr(
  id: string,
  author: GitIdentity,
  mergedAt: string,
  extra: Partial<RawPr> = {},
): RawPr {
  return {
    id,
    title: `PR ${id}`,
    state: "merged",
    source: "gh",
    author,
    mergedAt,
    commitCount: 1,
    branchCommits: [],
    landingCoAuthors: [],
    files: [file("src/features/a.ts", 100)],
    closesBug: false,
    ...extra,
  };
}

export function makeReview(
  prId: string,
  reviewer: GitIdentity,
  submittedAt: string,
  comments = 0,
  state = "APPROVED",
): RawReview {
  return { prId, reviewer, state, submittedAt, comments };
}

export function makeRaw(extra: Partial<RawData> = {}): RawData {
  return {
    head: "abc123",
    branch: "main",
    since: new Date(2026, 8, 1).toISOString(),
    until: new Date(2026, 10, 1).toISOString(),
    ghAvailable: true,
    commits: [],
    prs: [],
    reviews: [],
    survival: [],
    revertedPrIds: [],
    truncated: false,
    ...extra,
  };
}

export function makeResolver(
  raw: RawData,
  config: ContributorsConfig = DEFAULT_CONFIG,
) {
  return buildResolver(config, {
    merge: true,
    observedIdentities: observedIdentities(raw),
  });
}

export const withConfig = (
  extra: Partial<ContributorsConfig>,
): ContributorsConfig => ({ ...DEFAULT_CONFIG, ...extra });
