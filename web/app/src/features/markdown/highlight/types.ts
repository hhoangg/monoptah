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
/** Blocks at or under this size highlight on the main thread; larger ones in the Worker. */
export const INLINE_MAX_CHARS = 3_000;
