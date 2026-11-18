// Copied from src/features/files/editor/codeHighlightPlugin.ts at ca73ac2b
// Adapted: tokens instead of streamdown results; small blocks highlight inline (the engine
// is a lazy chunk), large ones in the Worker; no Worker (tests, old browsers) falls back inline.
import { normalizeLanguage } from "./languages";
import {
  INLINE_MAX_CHARS,
  MAX_HIGHLIGHT_CHARS,
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

function resetWorker() {
  worker?.terminate();
  worker = undefined;
  active = undefined;
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
  const inline = next.request.code.length <= INLINE_MAX_CHARS || typeof Worker === "undefined";
  if (inline) {
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
      }: MessageEvent<{ id: number; result?: HighlightResult | null; error?: string }>) => {
        const job = active;
        if (!job || data.id !== job.id) return;
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
