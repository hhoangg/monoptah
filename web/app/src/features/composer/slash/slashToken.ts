// The `/` token of a composer and the ranking of the menu rows. Both follow the native composer
// (`monocode-core` `composer::token_at` and `composer::score`) so the two menus agree on when they
// open and on what they list first.
import type { SlashCommand } from "@monoptah/protocol";

export interface SlashToken {
  /** Index of the `/` in the text. */
  start: number;
  /** End of the word the caret is in (the caret may sit in the middle of it). */
  end: number;
  /** What follows the `/`, up to the caret. */
  query: string;
}

/**
 * The slash command under the caret: a word that starts with `/` at a word boundary. Paths, URLs, a quoted
 * line (`> ...`) and anything with `@`, `.`, `\` or a second `/` never open the menu.
 */
export function slashTokenAt(text: string, caret: number): SlashToken | null {
  if (caret < 0 || caret > text.length) return null;
  let start = caret;
  while (start > 0 && !/\s/.test(text.charAt(start - 1))) start -= 1;
  const fragment = text.slice(start, caret);
  if (!fragment.startsWith("/")) return null;
  const query = fragment.slice(1);
  if (/[@\n\r/.\\]/.test(query)) return null;
  const before = text.slice(0, start).split("\n").pop() ?? "";
  if (before.trimStart().startsWith(">")) return null;
  let end = caret;
  while (end < text.length && !/\s/.test(text.charAt(end))) end += 1;
  return { start, end, query };
}

/** A fuzzy score, higher is better, `null` when `value` does not match: the same tiers as the native menu. */
export function scoreCommand(query: string, value: string): number | null {
  const q = query.toLowerCase();
  const v = value.toLowerCase();
  if (q === "") return 0;
  if (v === q) return 10_000;
  const leaf = v.slice(v.lastIndexOf("/") + 1);
  if (leaf.startsWith(q)) return 5_000 - v.length;
  const at = v.indexOf(q);
  if (at >= 0) return 3_000 - at - v.length;
  let rest = v;
  let gaps = 0;
  for (const char of q) {
    const found = rest.indexOf(char);
    if (found < 0) return null;
    gaps += found;
    rest = rest.slice(found + char.length);
  }
  return 1_000 - gaps - v.length;
}

/**
 * The rows that match `query`, best first. The bare `/` keeps the desktop's fixed order for the built-ins and puts
 * the skills after them; a query ranks everything by score and then by name.
 */
export function rankCommands(rows: readonly SlashCommand[], query: string): SlashCommand[] {
  const scored: { score: number; row: SlashCommand }[] = [];
  let order = 0;
  for (const row of rows) {
    const text = row.kind === "builtin" ? `${row.id} ${row.label}` : `${row.label} ${row.detail}`;
    const score = scoreCommand(query, text);
    if (row.kind === "builtin") order += 1;
    if (score === null) continue;
    const priority = row.kind === "builtin" ? (query === "" ? 100 - order : 100) : 0;
    scored.push({ score: score + priority, row });
  }
  scored.sort((a, b) => b.score - a.score || a.row.label.localeCompare(b.row.label));
  return scored.map((entry) => entry.row);
}
