/** Event the Rust sync jobs emit on every change of a job. */
export const SYNC_EVENT = "contributors://sync";

export type SyncStep =
  "counting" | "commits" | "pullRequests" | "github" | "survival";

export type SyncJobState =
  "queued" | "running" | "done" | "error" | "cancelled";

/** Mirrors `SyncEvent` in `src-tauri/src/contributors/jobs.rs`. */
export type SyncEvent = {
  /** The repository (same for every worktree of it). */
  repoKey: string;
  /** Folder name of the repository, for display. */
  project: string;
  /** The path the sync was requested from: the project path of the page. */
  root: string;
  branch: string;
  /** The period the job works on, in epoch seconds. */
  from: number;
  to: number;
  step: SyncStep;
  done: number;
  total: number;
  /** Weighted over all steps, 0 to 100. */
  percent: number;
  state: SyncJobState;
  error: string | null;
};

export const SYNC_STEP_LABEL: Record<SyncStep, string> = {
  counting: "Counting",
  commits: "Commits",
  pullRequests: "Pull requests",
  github: "GitHub",
  survival: "Survival",
};

/** A job that has not ended yet. */
export const isLive = (event: SyncEvent) =>
  event.state === "queued" || event.state === "running";
