// Copied from src/features/files/editor/codeHighlightEngine.ts at ca73ac2b
// Adapted: no @streamdown/code, fine-grained shiki core with the JavaScript regex engine,
// one github-light / github-dark pair, plain tokens out (never an HTML string).
import { createHighlighterCore, type GrammarState, type HighlighterCore } from "shiki/core";
import { createJavaScriptRegexEngine } from "shiki/engine/javascript";
import { loadLanguage, normalizeLanguage, type LanguageId } from "./languages";
import {
  MAX_HIGHLIGHT_CHARS,
  type CodeToken,
  type HighlightRequest,
  type HighlightResult,
} from "./types";

const MAX_ENTRIES = 100;
const MAX_CACHED_CHARS = 500_000;
const MAX_LINE_LENGTH = 2_000;

const THEMES = { light: "github-light", dark: "github-dark" } as const;

export interface HighlightEngine {
  /** Resolves to tokens, or null when the language is not in the curated set or the block is too large. */
  highlight(request: HighlightRequest): Promise<HighlightResult | null>;
  cachedResults(): number;
}

let highlighterPromise: Promise<HighlighterCore> | undefined;
let inlineEngine: HighlightEngine | undefined;

/** The main-thread engine, shared by every small block. */
export function highlightInline(request: HighlightRequest): Promise<HighlightResult | null> {
  inlineEngine ??= createHighlightEngine();
  return inlineEngine.highlight(request);
}

function sharedHighlighter(): Promise<HighlighterCore> {
  if (!highlighterPromise) {
    const created = Promise.all([
      import("shiki/themes/github-light.mjs"),
      import("shiki/themes/github-dark.mjs"),
    ]).then(([light, dark]) =>
      createHighlighterCore({
        themes: [light.default, dark.default],
        langs: [],
        engine: createJavaScriptRegexEngine({ forgiving: true }),
      }),
    );
    created.catch(() => {
      highlighterPromise = undefined;
    });
    highlighterPromise = created;
  }
  return highlighterPromise;
}

/**
 * Highlights with a bounded LRU and an exact key. Only the most recent fence keeps a
 * streaming checkpoint (its complete lines and grammar state); the unfinished line is
 * tokenized again because its syntax can change as characters arrive.
 */
export function createHighlightEngine(
  options: { maxEntries?: number; maxChars?: number } = {},
): HighlightEngine {
  const maxEntries = options.maxEntries ?? MAX_ENTRIES;
  const maxChars = options.maxChars ?? MAX_CACHED_CHARS;
  const results = new Map<string, HighlightResult>();
  const pending = new Map<string, Promise<HighlightResult | null>>();
  let cachedChars = 0;
  let checkpoint:
    | { lang: LanguageId; source: string; tokens: CodeToken[][]; state: GrammarState | undefined }
    | undefined;

  const remember = (key: string, result: HighlightResult) => {
    if (results.delete(key)) cachedChars -= key.length;
    if (key.length > maxChars) return;
    results.set(key, result);
    cachedChars += key.length;
    while (results.size > maxEntries || cachedChars > maxChars) {
      const oldest = results.keys().next().value;
      if (oldest === undefined) break;
      results.delete(oldest);
      cachedChars -= oldest.length;
    }
  };

  const convert = (
    highlighter: HighlighterCore,
    code: string,
    lang: LanguageId,
    state?: GrammarState,
  ) => {
    const out = highlighter.codeToTokens(code, {
      lang,
      themes: THEMES,
      defaultColor: false,
      grammarState: state,
      tokenizeMaxLineLength: MAX_LINE_LENGTH,
    });
    const tokens = out.tokens.map((line) =>
      line.map((token): CodeToken => {
        const style = token.htmlStyle as Record<string, string> | undefined;
        const converted: CodeToken = { content: token.content };
        if (style?.["--shiki-light"]) converted.light = style["--shiki-light"];
        if (style?.["--shiki-dark"]) converted.dark = style["--shiki-dark"];
        if (token.fontStyle && token.fontStyle > 0) converted.fontStyle = token.fontStyle;
        return converted;
      }),
    );
    return { tokens, state: out.grammarState };
  };

  const run = async (
    code: string,
    lang: LanguageId,
    key: string,
  ): Promise<HighlightResult | null> => {
    const highlighter = await sharedHighlighter();
    if (!highlighter.getLoadedLanguages().includes(lang)) {
      await highlighter.loadLanguage((await loadLanguage(lang)).default);
    }
    const previous =
      checkpoint?.lang === lang && code.startsWith(checkpoint.source) ? checkpoint : undefined;
    const offset = previous?.source.length ?? 0;
    const boundary = code.lastIndexOf("\n") + 1;
    let prefix = previous?.tokens ?? [];
    let state = previous?.state;
    if (boundary > offset) {
      const lineEnd = code[boundary - 2] === "\r" ? boundary - 2 : boundary - 1;
      const complete = convert(highlighter, code.slice(offset, lineEnd), lang, state);
      prefix = [...prefix, ...complete.tokens];
      state = complete.state;
    }
    const tail = convert(highlighter, code.slice(boundary), lang, state);
    const result: HighlightResult = { tokens: [...prefix, ...tail.tokens] };
    checkpoint =
      boundary > 0 && boundary <= maxChars
        ? { lang, source: code.slice(0, boundary), tokens: prefix, state }
        : undefined;
    remember(key, result);
    return result;
  };

  return {
    cachedResults: () => results.size,
    async highlight({ code, language }) {
      const lang = normalizeLanguage(language);
      if (!lang || code.length > MAX_HIGHLIGHT_CHARS) return null;
      const key = `${lang}\u0000${code}`;
      const cached = results.get(key);
      if (cached) {
        results.delete(key);
        results.set(key, cached);
        return cached;
      }
      let job = pending.get(key);
      if (!job) {
        job = run(code, lang, key).finally(() => pending.delete(key));
        pending.set(key, job);
      }
      return job;
    },
  };
}
