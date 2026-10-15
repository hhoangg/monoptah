export type GitIdentity = { name: string; email: string; login?: string };

/** Stable key of an identity: lowercased email, `gh:<login>`, or lowercased name. */
export type IdentityKey = string;

export type Person = { id: string; name: string; identities: string[] };

/** Per-file numstat of a unit, from `git ... --numstat -w -M`. Binary files omitted. Pure renames have 0/0 and are omitted. */
export type RawFile = { path: string; added: number; deleted: number };

export type RawCommit = {
  hash: string;
  subject: string;
  author: GitIdentity;
  committer: GitIdentity;
  authoredAt: string; // ISO 8601
  committedAt: string; // ISO 8601, used for all day bucketing (anti backdate)
  coAuthors: GitIdentity[]; // from `Co-authored-by:` trailers
  files: RawFile[];
  prId?: string; // set when this commit landed/belongs to a RawPr
};

export type RawBranchCommit = {
  authoredAt: string;
  committedAt: string;
  authors: GitIdentity[]; // author first, then co-authors
};

export type RawPr = {
  id: string; // GitHub PR number as string, or short landing hash when derived from git
  number?: number;
  title: string;
  state: "merged" | "closed"; // closed = closed without merge
  source: "gh" | "git";
  author: GitIdentity; // PR author (gh) or landing commit author (git)
  committer?: GitIdentity; // committer of the landing commit
  mergedAt?: string;
  closedAt?: string;
  mergeCommit?: string;
  commitCount: number; // >= 1
  branchCommits: RawBranchCommit[]; // may be shorter than commitCount when unknown (git squash fallback: empty)
  landingCoAuthors: GitIdentity[]; // Co-authored-by trailers on the landing commit
  files: RawFile[]; // FINAL diff that landed
  closesBug: boolean; // gh: closing issue labelled bug; git: false
};

export type RawReview = {
  prId: string;
  reviewer: GitIdentity; // login always set, email may be empty
  state: string; // APPROVED | COMMENTED | CHANGES_REQUESTED | ...
  submittedAt: string;
  comments: number; // count of comments with body > 20 chars (review body + inline when available)
};

/** Lines written by a PR that are still alive at HEAD. Only for PRs merged >= 30 days before `until`. */
export type RawSurvival = { prId: string; written: number; alive: number };

export type RawData = {
  head: string;
  branch: string;
  since: string;
  until: string; // ISO
  ghAvailable: boolean;
  ghNote?: string; // why gh was unavailable (not installed, not logged in, no GitHub remote)
  commits: RawCommit[]; // non-merge, patch-id deduped (later duplicates dropped)
  prs: RawPr[];
  reviews: RawReview[];
  survival: RawSurvival[];
  revertedPrIds: string[];
  truncated: boolean; // hit a safety cap
};

export type ContributorsConfig = {
  version: 1;
  season: "monthly"; // only monthly is implemented; field reserved
  weights: {
    delivery: number;
    review: number;
    care: number;
    consistency: number;
    breadth: number;
  };
  ignore: string[]; // globs
  excludeIdentities: string[]; // patterns on email or name, `*` wildcard, case-insensitive
  agentIdentities: string[]; // same pattern rules; agents win over exclude
  people: Person[]; // identities = lowercased emails, "gh:<login>", or names
  dismissedMerges: string[]; // suggestion ids the user dismissed
};

export type GhState = "complete" | "partial" | "unavailable";

/** A period read from the sync store (`contributors_load`). */
export type LoadResponse = {
  raw: RawData;
  /** The whole period is synced and nothing in it is missing. */
  covered: boolean;
  /**
   * Part of the period was never synced or its sync was cancelled. False
   * with `covered` false: a synced period that lacks some commits.
   */
  hasGaps: boolean;
  /** Commits of the period that the store does not have. */
  missingCommits: number;
  /** Epoch seconds of the last sync, or null when nothing is synced. */
  syncedAt: number | null;
  /** Branch head the synced period was last brought up to. */
  coverageHead: string | null;
  /** Branch head now. */
  head: string;
  ghState: GhState | null;
};

/** What syncing a period involves (`contributors_sync_estimate`). */
export type SyncEstimate = {
  commits: number;
  storedCommits: number;
  firstParent: number;
  /** Null when GitHub cannot say. */
  prs: number | null;
};

export type SyncMode = "full" | "topup" | "github";
