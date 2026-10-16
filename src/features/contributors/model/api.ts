import { invoke } from "@tauri-apps/api/core";
import type { LoadResponse, SyncEstimate, SyncMode } from "./types";
import type { SyncEvent } from "./syncEvents";

// Plain `invoke`: these commands read the local repository and are not routed
// to remote machines.

export type ContributorsBranches = { current: string; branches: string[] };
export type GitUser = { name: string; email: string };

/** The config file text, or null when the file does not exist. */
export function readConfig(cwd: string): Promise<string | null> {
  return invoke<string | null>("contributors_read_config", { cwd });
}

/** The repo `.mailmap` text, or null when the file does not exist. */
export function readMailmap(cwd: string): Promise<string | null> {
  return invoke<string | null>("contributors_read_mailmap", { cwd });
}

export function writeConfig(cwd: string, text: string): Promise<void> {
  return invoke<void>("contributors_write_config", { cwd, content: text });
}

/** One period of the sync store; nothing is synced or fetched. */
export function loadStored(
  cwd: string,
  { since, until, branch }: { since: string; until: string; branch?: string },
): Promise<LoadResponse> {
  return invoke<LoadResponse>("contributors_load", {
    cwd,
    branch: branch ?? null,
    since,
    until,
  });
}

type SyncWindowArgs = { from: string; to: string; branch?: string };

/** What syncing a period involves; cheap counts only. */
export function syncEstimate(
  cwd: string,
  { from, to, branch }: SyncWindowArgs,
): Promise<SyncEstimate> {
  return invoke<SyncEstimate>("contributors_sync_estimate", {
    cwd,
    branch: branch ?? null,
    from,
    to,
  });
}

/** Queues a sync; progress arrives as `SYNC_EVENT`. */
export function startSync(
  cwd: string,
  {
    from,
    to,
    branch,
    mode,
    oldHead,
  }: SyncWindowArgs & { mode: SyncMode; oldHead?: string },
): Promise<void> {
  return invoke<void>("contributors_sync_start", {
    cwd,
    branch: branch ?? null,
    from,
    to,
    mode,
    oldHead: oldHead ?? null,
  });
}

/** Stops the running sync of the repository and drops the ones queued behind it. */
export function cancelSync(cwd: string): Promise<void> {
  return invoke<void>("contributors_sync_cancel", { cwd });
}

/** The latest event of every running and queued job. */
export function syncStatus(): Promise<SyncEvent[]> {
  return invoke<SyncEvent[]>("contributors_sync_status");
}

/** Deletes everything synced for the repository. */
export function clearSynced(cwd: string): Promise<void> {
  return invoke<void>("contributors_clear", { cwd });
}

export function listBranches(cwd: string): Promise<ContributorsBranches> {
  return invoke<ContributorsBranches>("contributors_branches", { cwd });
}

/** `git config user.name` and `user.email`; empty strings when unset. */
export function gitUser(cwd: string): Promise<GitUser> {
  return invoke<GitUser>("contributors_git_user", { cwd });
}

/**
 * Committer date (ISO 8601) of the oldest root commit reachable from the
 * branch, or null for a repository without commits. Where All time starts.
 */
export function firstCommit(
  cwd: string,
  branch?: string,
): Promise<string | null> {
  return invoke<string | null>("contributors_first_commit", {
    cwd,
    branch: branch ?? null,
  });
}
