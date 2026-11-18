// Copied from src/shared/lib/fuzzy.ts at c579af2e
/**
 * Sequential fuzzy match: all query characters in order, with bonuses for
 * consecutive runs, path separators and camelCase boundaries.
 *
 * API (pure, no DOM):
 * - `fuzzyMatch(query, text)` -> `FuzzyHit | null`. A whitespace-separated query
 *   matches every token (AND); `positions` are sorted indexes into `text`.
 * - `scorePath(query, relative, name)` -> `FuzzyHit | null`. Prefers hits in the
 *   file name; `positions` always index `relative`.
 * - `rankBy(query, items, getText)` -> items sorted best first, with their hits.
 *   Pass `positions` to `MatchText` (`@/ui/MatchText`) to highlight a result.
 */

export type FuzzyHit = {
  score: number;
  positions: number[];
};

export function fuzzyMatch(query: string, text: string): FuzzyHit | null {
  const tokens = query.trim().split(/\s+/).filter(Boolean);
  if (tokens.length === 0) return { score: 0, positions: [] };
  if (tokens.length === 1) return matchToken(tokens[0]!, text);

  const positions: number[] = [];
  let score = 0;
  for (const token of tokens) {
    const hit = matchToken(token, text);
    if (!hit) return null;
    score += hit.score;
    for (const index of hit.positions) positions.push(index);
  }
  positions.sort((a, b) => a - b);
  return { score, positions };
}

function matchToken(query: string, text: string): FuzzyHit | null {
  if (!query) return { score: 0, positions: [] };

  const needle = query.toLowerCase();
  const hay = text.toLowerCase();
  const positions: number[] = [];
  let score = 0;
  let consecutive = 0;
  let qi = 0;

  for (let i = 0; i < hay.length && qi < needle.length; i++) {
    if (hay[i] !== needle[qi]) {
      consecutive = 0;
      continue;
    }
    positions.push(i);
    consecutive += 1;
    score += 1 + consecutive * 4;
    const prev = text[i - 1] ?? "";
    const cur = text[i] ?? "";
    if (i === 0 || isBreak(prev)) score += 14;
    else if (isUpper(cur) && !isUpper(prev)) score += 10;
    qi += 1;
  }

  if (qi !== needle.length) return null;
  score -= hay.length - needle.length;
  return { score, positions };
}

function isBreak(ch: string): boolean {
  return ch === "/" || ch === "\\" || ch === "-" || ch === "_" || ch === "." || ch === " ";
}

function isUpper(ch: string): boolean {
  return ch >= "A" && ch <= "Z";
}

/** Prefer filename hits over directory-only hits. Positions index `relative`. */
export function scorePath(query: string, relative: string, name: string): FuzzyHit | null {
  const needle = query.trim();
  if (!needle) return { score: 0, positions: [] };

  const offset = Math.max(0, relative.length - name.length);
  const nameHit = fuzzyMatch(needle, name);
  if (nameHit) {
    return {
      score: nameHit.score + 400,
      positions: nameHit.positions.map((index) => index + offset),
    };
  }

  return fuzzyMatch(needle, relative);
}

/** Added for the web app: filter and sort `items` by fuzzy score (stable for equal scores). */
export function rankBy<T>(
  query: string,
  items: readonly T[],
  getText: (item: T) => string,
): { item: T; hit: FuzzyHit }[] {
  const out: { item: T; hit: FuzzyHit; index: number }[] = [];
  items.forEach((item, index) => {
    const hit = fuzzyMatch(query, getText(item));
    if (hit) out.push({ item, hit, index });
  });
  out.sort((a, b) => b.hit.score - a.hit.score || a.index - b.index);
  return out.map(({ item, hit }) => ({ item, hit }));
}
