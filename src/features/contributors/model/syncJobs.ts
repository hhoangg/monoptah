import { listen } from "@tauri-apps/api/event";
import { useSyncExternalStore } from "react";
import {
  cancelSync as cancelSyncCommand,
  startSync as startSyncCommand,
  syncStatus,
} from "./api";
import { SYNC_EVENT, isLive } from "./syncEvents";
import type { SyncEvent } from "./syncEvents";
import type { SyncMode } from "./types";

/**
 * The one place the webview learns about contributors sync jobs.
 *
 * It listens to `SYNC_EVENT` once for the whole app, seeds itself from
 * `contributors_sync_status`, and keeps the latest event of every job. The
 * Contributors page and the footer indicator both read it.
 *
 * A job entry can only be wrong when an event was missed (the job ended
 * before `listen` registered, a status answer arrived after the `done`
 * event, or Rust dropped a duplicate request without an event). Two things
 * keep that from leaving a job on screen forever:
 *
 * - An ended job leaves a tombstone, so a status answer that was asked for
 *   before it ended cannot bring it back.
 * - `reconcile` asks Rust what really runs and drops live entries it does
 *   not list. It runs after listening starts, after every start and cancel,
 *   and when the window gets focus.
 */

/** Latest event of every job, by `jobKey`. Replaced (never mutated) on change. */
export type SyncJobs = Readonly<Record<string, SyncEvent>>;

/** Identity of a job as the page sees it: project path, branch, and window in epoch seconds. */
export function jobKey(
  root: string,
  branch: string,
  from: number,
  to: number,
): string {
  return `${root}\u0000${branch}\u0000${from}\u0000${to}`;
}

export const eventKey = (event: SyncEvent) =>
  jobKey(event.root, event.branch, event.from, event.to);

const EMPTY: SyncJobs = {};
let jobs: SyncJobs = EMPTY;
/** Marker of the last change of a key. */
const touched = new Map<string, number>();
/** Marker at which a key ended. A status answer older than it must not revive the key. */
const tombstones = new Map<string, number>();
let marker = 0;
const listeners = new Set<() => void>();
const endedListeners = new Set<(event: SyncEvent) => void>();

let started: Promise<void> | null = null;
let stopEvents: (() => void) | undefined;
let removeFocus: (() => void) | undefined;
let epoch = 0;

function publish(next: SyncJobs) {
  jobs = next;
  listeners.forEach((listener) => listener());
}

function put(event: SyncEvent) {
  const key = eventKey(event);
  touched.set(key, (marker += 1));
  publish({ ...jobs, [key]: event });
}

function remove(key: string) {
  if (!(key in jobs)) return;
  const { [key]: _gone, ...rest } = jobs;
  publish(rest);
}

/** Applies an event from Rust: the authority on its own job. */
function apply(event: SyncEvent) {
  const key = eventKey(event);
  if (isLive(event)) {
    tombstones.delete(key);
    put(event);
    return;
  }
  if (event.state === "error") {
    // Kept so the page can say why; it is not a running job any more.
    tombstones.set(key, (marker += 1));
    put(event);
  } else {
    tombstones.set(key, (marker += 1));
    touched.delete(key);
    remove(key);
  }
  endedListeners.forEach((listener) => listener(event));
}

/** The jobs now, for code outside React. */
export const getSyncJobs = (): SyncJobs => jobs;

export function subscribeSyncJobs(listener: () => void): () => void {
  listeners.add(listener);
  return () => listeners.delete(listener);
}

/** Re-renders with the latest jobs. */
export function useSyncJobs(): SyncJobs {
  return useSyncExternalStore(subscribeSyncJobs, getSyncJobs, getSyncJobs);
}

/**
 * Calls `listener` when a job ended (done, cancelled, failed), or vanished
 * from Rust's list, so the caller can read what it stored.
 */
export function onSyncJobEnded(
  listener: (event: SyncEvent) => void,
): () => void {
  endedListeners.add(listener);
  return () => endedListeners.delete(listener);
}

/**
 * Makes `jobs` match what Rust lists: live entries it does not know are
 * dropped, listed ones are added. Entries changed (or ended) after the
 * question was asked are left alone, since the answer may predate them.
 */
export async function reconcile(): Promise<void> {
  const asked = marker;
  const mine = epoch;
  let running: SyncEvent[];
  try {
    running = await syncStatus();
  } catch {
    return;
  }
  if (mine !== epoch) return;
  const listed = new Set<string>();
  for (const event of running) {
    const key = eventKey(event);
    listed.add(key);
    if ((tombstones.get(key) ?? 0) > asked) continue;
    if ((touched.get(key) ?? 0) > asked) continue;
    if (jobs[key] && jobs[key].state === event.state) continue;
    put(event);
  }
  for (const [key, entry] of Object.entries(jobs)) {
    if (!isLive(entry) || listed.has(key)) continue;
    if ((touched.get(key) ?? 0) > asked) continue;
    touched.delete(key);
    remove(key);
    endedListeners.forEach((listener) =>
      listener({ ...entry, state: "cancelled" }),
    );
  }
}

/**
 * Starts listening (once) and seeds from Rust. Safe to call from every page
 * mount; resolves when the first reconcile is done.
 */
export function ensureSyncJobs(): Promise<void> {
  if (started) return started;
  const mine = epoch;
  const onFocus = () => void reconcile();
  window.addEventListener("focus", onFocus);
  removeFocus = () => window.removeEventListener("focus", onFocus);
  started = listen<SyncEvent>(SYNC_EVENT, (e) => apply(e.payload))
    .then((stop) => {
      if (mine !== epoch) stop();
      else stopEvents = stop;
    })
    .catch(() => undefined)
    // Only now can no event be missed, so only now is a status answer
    // complete enough to drop entries by.
    .then(() => reconcile());
  return started;
}

const isoSeconds = (date: Date) => date.toISOString().replace(/\.\d{3}Z$/, "Z");
const epochSeconds = (date: Date) => Math.floor(date.getTime() / 1000);

export type StartJob = {
  /** The branch name Rust resolves and reports in its events. */
  branchName: string;
  /** The branch the user picked; undefined follows the checked-out one. */
  branch?: string;
  window: { from: Date; to: Date };
  mode: SyncMode;
  oldHead?: string;
};

/**
 * Queues a job. A queued entry appears at once, and is replaced by the real
 * events; if Rust refuses or silently drops the request it goes away again.
 * A refusal shows as an error entry.
 */
export async function startSyncJob(
  cwd: string,
  { branchName, branch, window: span, mode, oldHead }: StartJob,
): Promise<void> {
  void ensureSyncJobs();
  const from = epochSeconds(span.from);
  const to = epochSeconds(span.to);
  const base: SyncEvent = {
    repoKey: "",
    project: "",
    root: cwd,
    branch: branchName,
    from,
    to,
    step: "counting",
    done: 0,
    total: 0,
    percent: 0,
    state: "queued",
    error: null,
  };
  tombstones.delete(eventKey(base));
  put(base);
  try {
    await startSyncCommand(cwd, {
      from: isoSeconds(span.from),
      to: isoSeconds(span.to),
      branch,
      mode,
      oldHead,
    });
  } catch (error) {
    put({
      ...base,
      state: "error",
      error: error instanceof Error ? error.message : String(error),
    });
  }
  // A request Rust dropped as a duplicate (or one that failed) has no job
  // behind it; this removes the queued entry made above.
  await reconcile();
}

/** Stops the jobs of the project's repository, then checks what is still running. */
export async function cancelSyncJobs(cwd: string): Promise<void> {
  try {
    await cancelSyncCommand(cwd);
  } finally {
    await reconcile();
  }
}

/** Forgets every entry of a project, e.g. after its synced data was cleared. */
export function forgetSyncJobs(root: string) {
  const prefix = `${root}\u0000`;
  const kept = Object.fromEntries(
    Object.entries(jobs).filter(([key]) => !key.startsWith(prefix)),
  );
  if (Object.keys(kept).length !== Object.keys(jobs).length) publish(kept);
}

/** Test hook: back to a fresh start, listeners and all. */
export function resetSyncJobs() {
  epoch += 1;
  stopEvents?.();
  stopEvents = undefined;
  removeFocus?.();
  removeFocus = undefined;
  started = null;
  jobs = EMPTY;
  touched.clear();
  tombstones.clear();
  marker = 0;
  listeners.forEach((listener) => listener());
}
