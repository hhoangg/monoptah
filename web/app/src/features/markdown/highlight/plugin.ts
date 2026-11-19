// Copied from src/features/files/editor/codeHighlightPlugin.ts at ca73ac2b
// Adapted: tokens instead of streamdown results. Agent text is untrusted and a grammar can backtrack for
// seconds, so blocks highlight in the Worker with a time limit (the Worker is terminated and the block stays
// plain text). With no Worker (tests, old browsers) only tiny blocks highlight inline.
import { normalizeLanguage } from "./languages";
import {
  INLINE_MAX_CHARS,
  MAX_HIGHLIGHT_CHARS,
  WORKER_JOB_TIMEOUT_MS,
  WORKER_START_TIMEOUT_MS,
  type HighlightRequest,
  type HighlightResult,
} from "./types";

type Callback = (result: HighlightResult) => void;
type Job = {
  id: number;
  key: string;
  request: HighlightRequest;
  callbacks: Set<Callback>;
  priority: number;
};

const MAX_CHARS = 500_000;
const MAX_RESULTS = 100;
const results = new Map<string, HighlightResult>();
const jobs = new Map<string, Job>();
let chars = 0;
let sequence = 0;
let active: Job | undefined;
let worker: Worker | undefined;
let workerReady = false;
/** Blocks whose job hit the time limit: they stay plain text and are not tried again. */
const timedOut = new Set<string>();
let watchdog: ReturnType<typeof setTimeout> | undefined;
let cancellationScheduled = false;

function keyFor(request: HighlightRequest) {
  return `${normalizeLanguage(request.language) ?? ""}\0${request.code}`;
}

/** Whether a fence tag is in the curated language set. */
export function supportsLanguage(language: string): boolean {
  return normalizeLanguage(language) !== undefined;
}

function remember(key: string, result: HighlightResult) {
  if (key.length > MAX_CHARS) return;
  if (results.delete(key)) chars -= key.length;
  results.set(key, result);
  chars += key.length;
  while (results.size > MAX_RESULTS || chars > MAX_CHARS) {
    const oldest = results.keys().next().value;
    if (oldest === undefined) break;
    chars -= oldest.length;
    results.delete(oldest);
  }
}

/** The tokens for a block already highlighted, without scheduling any work. */
export function cachedHighlight(request: HighlightRequest): HighlightResult | undefined {
  return results.get(keyFor(request));
}

function clearWatchdog() {
  if (watchdog !== undefined) clearTimeout(watchdog);
  watchdog = undefined;
}

function resetWorker() {
  clearWatchdog();
  worker?.terminate();
  worker = undefined;
  workerReady = false;
  active = undefined;
}

/** Gives up on the job the Worker is running: the Worker goes, the block stays plain text, the next job starts. */
function armWatchdog(job: Job, limit: number) {
  clearWatchdog();
  watchdog = setTimeout(() => {
    watchdog = undefined;
    if (active !== job) return;
    resetWorker();
    jobs.delete(job.key);
    if (timedOut.size >= MAX_RESULTS) timedOut.clear();
    timedOut.add(job.key);
    pump();
  }, limit);
}

function finish(job: Job, result: HighlightResult | null | undefined) {
  jobs.delete(job.key);
  if (result && job.callbacks.size) {
    remember(job.key, result);
    for (const callback of job.callbacks) callback(result);
  }
}

function pump() {
  if (active || !jobs.size) return;
  const next = [...jobs.values()]
    .filter((job) => job.callbacks.size)
    .sort((a, b) => a.priority - b.priority || a.id - b.id)[0];
  if (!next) return;
  active = next;
  if (typeof Worker === "undefined") {
    if (next.request.code.length > INLINE_MAX_CHARS) {
      // No Worker to stop a runaway grammar: a block this size stays plain text.
      active = undefined;
      jobs.delete(next.key);
      pump();
      return;
    }
    void import("./engine")
      .then(({ highlightInline }) => highlightInline(next.request))
      .then(
        (result) => finish(next, result),
        () => jobs.delete(next.key),
      )
      .finally(() => {
        if (active === next) active = undefined;
        pump();
      });
    return;
  }
  try {
    if (!worker) {
      const created = new Worker(new URL("./highlight.worker.ts", import.meta.url), {
        type: "module",
      });
      created.onmessage = ({
        data,
      }: MessageEvent<{ ready?: boolean; id?: number; result?: HighlightResult | null; error?: string }>) => {
        if (worker !== created) return;
        if (data.ready) {
          workerReady = true;
          // The engine is loaded: from here the job has its own time limit.
          if (active) armWatchdog(active, WORKER_JOB_TIMEOUT_MS);
          return;
        }
        const job = active;
        if (!job || data.id !== job.id) return;
        clearWatchdog();
        active = undefined;
        finish(job, data.result);
        pump();
      };
      created.onerror = () => {
        if (worker !== created) return;
        const failed = active;
        resetWorker();
        if (failed) jobs.delete(failed.key);
        pump();
      };
      worker = created;
    }
    armWatchdog(next, workerReady ? WORKER_JOB_TIMEOUT_MS : WORKER_START_TIMEOUT_MS);
    worker.postMessage({ id: next.id, request: next.request });
  } catch {
    const failed = active;
    resetWorker();
    if (failed) jobs.delete(failed.key);
  }
}

function cancelAbandonedWork() {
  if (cancellationScheduled || !active || active.callbacks.size || !worker) return;
  cancellationScheduled = true;
  // React removes the old subscription before subscribing to a growing fence. Let that
  // replacement arrive before discarding the worker's grammar checkpoint, otherwise every
  // streamed chunk restarts the tokenizer.
  queueMicrotask(() => {
    cancellationScheduled = false;
    const abandoned = active;
    if (!abandoned || abandoned.callbacks.size) return;
    const language = normalizeLanguage(abandoned.request.language);
    const continued = [...jobs.values()].some(
      (job) =>
        job !== abandoned &&
        job.callbacks.size > 0 &&
        normalizeLanguage(job.request.language) === language &&
        job.request.code.startsWith(abandoned.request.code),
    );
    if (continued) return;
    resetWorker();
    jobs.delete(abandoned.key);
    pump();
  });
}

/**
 * Asks for tokens; the callback gets them when ready (at once when cached). Returns a
 * cancel function. Unsupported languages and oversized blocks never call back.
 */
export function requestCodeHighlight(
  request: HighlightRequest,
  callback: Callback,
  priority = 0,
): () => void {
  if (!supportsLanguage(request.language) || request.code.length > MAX_HIGHLIGHT_CHARS) {
    return () => {};
  }
  const key = keyFor(request);
  if (timedOut.has(key)) return () => {};
  const cached = results.get(key);
  if (cached) {
    results.delete(key);
    results.set(key, cached);
    callback(cached);
    return () => {};
  }
  let job = jobs.get(key);
  if (!job) {
    job = { id: ++sequence, key, request, callbacks: new Set(), priority };
    jobs.set(key, job);
  }
  job.priority = Math.min(job.priority, priority);
  job.callbacks.add(callback);
  pump();
  const owned = job;
  return () => {
    owned.callbacks.delete(callback);
    if (!owned.callbacks.size && active !== owned) jobs.delete(key);
    cancelAbandonedWork();
  };
}

/** Forgets the results, the queue and the Worker. For tests. */
export function resetHighlighting() {
  resetWorker();
  results.clear();
  jobs.clear();
  timedOut.clear();
  chars = 0;
}
