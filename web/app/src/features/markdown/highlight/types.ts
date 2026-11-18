/** One highlighted run. Colours are hex strings from the two fixed shiki themes. */
export interface CodeToken {
  content: string;
  light?: string;
  dark?: string;
  /** Shiki FontStyle bit flags: 1 italic, 2 bold, 4 underline. */
  fontStyle?: number;
}

export interface HighlightResult {
  tokens: CodeToken[][];
}

export interface HighlightRequest {
  code: string;
  /** A fence tag; resolved to the curated set by the engine. */
  language: string;
}

/** Past this many characters a block is shown as plain text. */
export const MAX_HIGHLIGHT_CHARS = 200_000;
/**
 * Highlighting runs in the Worker, where a runaway grammar can be stopped. Only when the browser has no Worker
 * (and in tests) does a block of at most this many characters highlight on the main thread; larger ones stay plain.
 */
export const INLINE_MAX_CHARS = 300;
/** A Worker job that runs longer than this is abandoned: the Worker is terminated and the block stays plain text. */
export const WORKER_JOB_TIMEOUT_MS = 1_500;
/** The Worker has this long to start (it loads the engine) before the first job counts as failed. */
export const WORKER_START_TIMEOUT_MS = 10_000;
