// A small Markdown reader for agent replies: the subset agents actually write.
// Ported from mobile/src/features/sessions/markdownParser.ts. It produces an AST only; raw
// HTML is never interpreted (it stays literal text) and images are shown as their alt text.
// Hardened for hostile input: bounded nesting, bounded scans, no backtracking-prone patterns.

export type Inline =
  /** `word`: the place of its first word in the reply, set by numberWords. */
  | { type: "text"; text: string; word?: number }
  | { type: "code"; text: string }
  | { type: "strong"; children: Inline[] }
  | { type: "em"; children: Inline[] }
  | { type: "strike"; children: Inline[] }
  | { type: "link"; href: string; children: Inline[] };

export type MarkdownBlock =
  | { type: "paragraph"; inline: Inline[] }
  | { type: "heading"; level: number; inline: Inline[] }
  | { type: "code"; lang: string; text: string }
  | { type: "list"; ordered: boolean; start: number; items: MarkdownBlock[][] }
  | { type: "quote"; children: MarkdownBlock[] }
  | { type: "table"; rows: Inline[][][]; truncated?: boolean }
  | { type: "rule" };

const FENCE = /^ {0,3}(`{3,}|~{3,})\s*([\w+-]*)/;
const HEADING = /^ {0,3}(#{1,6})\s+(.*)$/;
const RULE = /^ {0,3}([-*_])(\s*\1){2,}\s*$/;
const ITEM = /^(\s*)([-*+]|\d{1,9}[.)])\s+(.*)$/;
const QUOTE = /^ {0,3}>\s?(.*)$/;
/** Lines longer than this never start a table (the divider pattern is not linear on long runs of space). */
const MAX_TABLE_LINE = 2000;
/** A table keeps at most this many rows and columns; the rest is dropped and the table says so (W13-6). */
export const MAX_TABLE_ROWS = 200;
export const MAX_TABLE_COLUMNS = 50;
/** Block and inline nesting past this depth is read as plain text. */
const MAX_DEPTH = 24;
/** How far an emphasis, code or link scan looks ahead. */
const MAX_SPAN = 5000;
const TABLE_DIVIDER = /^\s*\|?\s*:?-+:?\s*(\|\s*:?-+:?\s*)+\|?\s*$/;

/** A run of text split into words and the space between them. */
export function splitWords(text: string): string[] {
  return text.split(/(\s+)/).filter(Boolean);
}

/**
 * Numbers the words a reply shows, in reading order, and returns how many
 * there are. Text only grows as it streams, so a word keeps its number from
 * one render to the next and only new words fade. Links and code are read
 * whole and appear as they are, as on desktop.
 */
export function numberWords(blocks: MarkdownBlock[]): {
  count: number;
  ranges: [number, number][];
} {
  let count = 0;
  const inline = (nodes: Inline[]) => {
    for (const node of nodes) {
      if (node.type === "text") {
        node.word = count;
        count += splitWords(node.text).filter((part) => !/^\s/.test(part)).length;
      } else if (node.type === "strong" || node.type === "em" || node.type === "strike")
        inline(node.children);
    }
  };
  const block = (node: MarkdownBlock) => {
    switch (node.type) {
      case "paragraph":
      case "heading":
        inline(node.inline);
        break;
      case "list":
        node.items.forEach((item) => item.forEach(block));
        break;
      case "quote":
        node.children.forEach(block);
        break;
      case "table":
        node.rows.forEach((row) => row.forEach(inline));
        break;
    }
  };
  const ranges = blocks.map((node): [number, number] => {
    const start = count;
    block(node);
    return [start, count];
  });
  return { count, ranges };
}

export function parseMarkdown(source: string): MarkdownBlock[] {
  return parseBlocks(source.replace(/\r\n?/g, "\n").split("\n"), 0);
}

/** A heading's text without its closing hashes. */
function headingText(raw: string): string {
  let end = raw.trimEnd().length;
  while (end > 0 && raw[end - 1] === "#") end -= 1;
  return raw.slice(0, end).trimEnd();
}

function parseBlocks(lines: string[], depth: number): MarkdownBlock[] {
  const blocks: MarkdownBlock[] = [];
  let index = 0;
  while (index < lines.length) {
    const line = lines[index] ?? "";
    if (!line.trim()) {
      index += 1;
      continue;
    }

    const fence = FENCE.exec(line);
    if (fence) {
      const close = fence[1] ?? "```";
      const body: string[] = [];
      index += 1;
      // An unclosed fence, as while the reply streams, runs to the end.
      while (index < lines.length && !(lines[index] ?? "").trimStart().startsWith(close)) {
        body.push(lines[index] ?? "");
        index += 1;
      }
      index += 1;
      blocks.push({ type: "code", lang: fence[2] ?? "", text: body.join("\n") });
      continue;
    }

    const heading = HEADING.exec(line);
    if (heading) {
      blocks.push({
        type: "heading",
        level: (heading[1] ?? "#").length,
        inline: parseInline(headingText(heading[2] ?? "")),
      });
      index += 1;
      continue;
    }

    if (RULE.test(line)) {
      blocks.push({ type: "rule" });
      index += 1;
      continue;
    }

    if (depth < MAX_DEPTH && QUOTE.test(line)) {
      const body: string[] = [];
      while (index < lines.length && (lines[index] ?? "").trim()) {
        const current = lines[index] ?? "";
        const quoted = QUOTE.exec(current);
        body.push(quoted ? (quoted[1] ?? "") : current);
        index += 1;
      }
      blocks.push({ type: "quote", children: parseBlocks(body, depth + 1) });
      continue;
    }

    const divider = lines[index + 1] ?? "";
    if (line.includes("|") && divider.length <= MAX_TABLE_LINE && TABLE_DIVIDER.test(divider)) {
      const header = cells(line);
      const rows = [header.cells];
      let truncated = header.truncated;
      index += 2;
      while (index < lines.length && (lines[index] ?? "").includes("|") && (lines[index] ?? "").trim()) {
        if (rows.length < MAX_TABLE_ROWS) {
          const row = cells(lines[index] ?? "");
          rows.push(row.cells);
          truncated ||= row.truncated;
        } else truncated = true;
        index += 1;
      }
      blocks.push({ type: "table", rows, ...(truncated ? { truncated: true } : {}) });
      continue;
    }

    if (depth < MAX_DEPTH && ITEM.test(line)) {
      const [list, next] = parseList(lines, index, depth);
      blocks.push(list);
      index = next;
      continue;
    }

    // The first line always belongs to the paragraph, so the loop always advances.
    const paragraph: string[] = [];
    while (index < lines.length) {
      const current = lines[index] ?? "";
      if (paragraph.length > 0) {
        if (
          !current.trim() ||
          FENCE.test(current) ||
          HEADING.test(current) ||
          (depth < MAX_DEPTH && QUOTE.test(current)) ||
          ITEM.test(current)
        ) {
          break;
        }
      }
      paragraph.push(current.trim());
      index += 1;
    }
    blocks.push({ type: "paragraph", inline: parseInline(paragraph.join("\n")) });
  }
  return blocks;
}

function parseList(lines: string[], from: number, depth: number): [MarkdownBlock, number] {
  const first = ITEM.exec(lines[from] ?? "");
  const indent = (first?.[1] ?? "").length;
  const marker = first?.[2] ?? "-";
  const ordered = /\d/.test(marker);
  const items: string[][] = [];
  const push = (value: string) => {
    const current = items[items.length - 1];
    if (current) current.push(value);
    else items.push([value]);
  };
  let index = from;
  while (index < lines.length) {
    const line = lines[index] ?? "";
    const item = ITEM.exec(line);
    if (item && (item[1] ?? "").length === indent && /\d/.test(item[2] ?? "") === ordered) {
      items.push([item[3] ?? ""]);
      index += 1;
      continue;
    }
    const blank = !line.trim();
    const nested = line.length - line.trimStart().length > indent;
    // A blank line ends the list unless the next line carries on inside it.
    if (blank) {
      const next = lines[index + 1] ?? "";
      const nextItem = ITEM.exec(next);
      if (
        (nextItem && (nextItem[1] ?? "").length === indent) ||
        (next.trim() && next.length - next.trimStart().length > indent)
      ) {
        push("");
        index += 1;
        continue;
      }
      break;
    }
    if (nested) {
      push(line.slice(Math.min(indent + 2, line.length - line.trimStart().length)));
      index += 1;
      continue;
    }
    // A lazy continuation of the item's paragraph.
    if (!item && !FENCE.test(line) && !HEADING.test(line)) {
      push(line.trim());
      index += 1;
      continue;
    }
    break;
  }
  // The first line is a list item by construction, but never return without consuming it.
  if (index === from) index += 1;
  return [
    {
      type: "list",
      ordered,
      start: ordered ? parseInt(marker, 10) : 1,
      items: items.map((body) => parseBlocks(body, depth + 1)),
    },
    index,
  ];
}

function cells(row: string): { cells: Inline[][]; truncated: boolean } {
  const all = row
    .trim()
    .replace(/^\|/, "")
    .replace(/\|$/, "")
    .split("|");
  return { cells: all.slice(0, MAX_TABLE_COLUMNS).map((cell) => parseInline(cell.trim())), truncated: all.length > MAX_TABLE_COLUMNS };
}

const DELIMITERS: [string, "strong" | "em" | "strike"][] = [
  ["**", "strong"],
  ["__", "strong"],
  ["~~", "strike"],
  ["*", "em"],
  ["_", "em"],
];

/** Emphasis close-searches one top-level inline parse may make before emphasis reads as plain text. */
const MAX_SCAN_STEPS = 100_000;
let scanBudget = MAX_SCAN_STEPS;

const MAX_LABEL = 500;
const MAX_DESTINATION = 2000;
const MAX_GROUP = 200;
const MAX_TITLE = 500;

/** Per `parseInline` call: the start of the last destination scan that failed (every `[` of a run shares one `](`). */
interface LinkMemo {
  failed: number;
}

/**
 * `[label](destination "title")` at `open` (the index of `[`), without a backtracking regex (W13-6 / N1). The
 * destination allows one level of balanced parentheses, as in `[x](https://a.dev/f_(1))`. Every step is a forward scan
 * bounded by the limits above, so a hostile input cannot make it quadratic. Returns the label, the destination and
 * the end index, or null.
 */
function matchLink(text: string, open: number, minLabel: number, memo: LinkMemo): { label: string; href: string; end: number } | null {
  const close = text.indexOf("]", open + 1);
  if (close === -1 || close - open - 1 < minLabel || close - open - 1 > MAX_LABEL || text[close + 1] !== "(") return null;
  const start = close + 2;
  if (start === memo.failed) return null;
  let at = start;
  let units = 0;
  while (at < text.length && units < MAX_DESTINATION) {
    const char = text[at] as string;
    if (char === "(") {
      let inner = at + 1;
      while (inner < text.length && inner - at - 1 <= MAX_GROUP && !/[()\s]/.test(text[inner] as string)) inner += 1;
      if (text[inner] !== ")" || inner - at - 1 > MAX_GROUP) break;
      at = inner + 1;
    } else if (char === ")" || /\s/.test(char)) break;
    else at += 1;
    units += 1;
  }
  const fail = (): null => {
    memo.failed = start;
    return null;
  };
  if (units === 0) return fail();
  const href = text.slice(start, at);
  let end = at;
  if (/\s/.test(text[end] ?? "")) {
    let quote = end;
    while (/\s/.test(text[quote] ?? "")) quote += 1;
    if (text[quote] !== '"') return fail();
    const closeQuote = text.indexOf('"', quote + 1);
    if (closeQuote === -1 || closeQuote - quote - 1 > MAX_TITLE) return fail();
    end = closeQuote + 1;
  }
  if (text[end] !== ")") return fail();
  return { label: text.slice(open + 1, close), href, end: end + 1 };
}
const BARE_URL = /https?:\/\/[^\s<>()]{1,2000}/y;

function sticky(pattern: RegExp, text: string, index: number): RegExpExecArray | null {
  pattern.lastIndex = index;
  return pattern.exec(text);
}

/** `inLink`: the text is a link's label, where a bare URL is not turned into a second (nested) link. */
export function parseInline(text: string, depth = 0, inLink = false): Inline[] {
  if (depth === 0) scanBudget = MAX_SCAN_STEPS;
  if (depth > MAX_DEPTH) return text ? [{ type: "text", text }] : [];
  const out: Inline[] = [];
  let plain = "";
  const flush = () => {
    if (plain) out.push({ type: "text", text: plain });
    plain = "";
  };
  let index = 0;
  // Next `]` at or after the last look-up: a link or image label is at most 500 characters, so a `[` whose next `]`
  // is farther away (or missing) cannot start one. Without this a run of `[` costs 500 steps each (W13-6).
  const memo: LinkMemo = { failed: -1 };
  let nextClose = -2;
  const labelCanClose = (from: number): boolean => {
    if (nextClose !== -1 && nextClose < from) nextClose = text.indexOf("]", from);
    else if (nextClose === -2) nextClose = text.indexOf("]", from);
    return nextClose !== -1 && nextClose - from <= 501;
  };
  while (index < text.length) {
    const char = text[index];
    if (char === "\\" && index + 1 < text.length && /[\\`*_~[\]()#>+-.!|]/.test(text[index + 1] ?? "")) {
      plain += text[index + 1];
      index += 2;
      continue;
    }
    if (char === "`") {
      let run = index;
      while (text[run] === "`") run += 1;
      const ticks = text.slice(index, run);
      const close = text.indexOf(ticks, run);
      if (close > 0 && close - run <= MAX_SPAN) {
        flush();
        out.push({ type: "code", text: text.slice(index + ticks.length, close).trim() });
        index = close + ticks.length;
        continue;
      }
      // No matching close: the whole run is literal text (as in CommonMark), so a long run is read once.
      plain += ticks;
      index = run;
      continue;
    }
    if (char === "[" && labelCanClose(index + 1)) {
      const link = matchLink(text, index, 1, memo);
      if (link) {
        flush();
        out.push({ type: "link", href: link.href, children: parseInline(link.label, depth + 1, true) });
        index = link.end;
        continue;
      }
    }
    if (char === "!" && text[index + 1] === "[" && labelCanClose(index + 2)) {
      // Images are never loaded: the alt text is shown instead.
      const image = matchLink(text, index + 1, 0, memo);
      if (image) {
        plain += image.label || "image";
        index = image.end;
        continue;
      }
    }
    if (
      !inLink &&
      char === "h" &&
      /^https?:\/\//.test(text.slice(index, index + 8)) &&
      !/\w/.test(text[index - 1] ?? "")
    ) {
      const found = sticky(BARE_URL, text, index);
      // Trailing punctuation belongs to the sentence, not the URL.
      let length = found ? found[0].length : 0;
      while (length > 0 && /[.,;:!?'"]/.test(text[index + length - 1] ?? "")) length -= 1;
      const url = found && length > 8 ? text.slice(index, index + length) : "";
      if (url) {
        flush();
        out.push({ type: "link", href: url, children: [{ type: "text", text: url }] });
        index += url.length;
        continue;
      }
    }
    const delimiter = DELIMITERS.find(([mark]) => text.startsWith(mark, index));
    if (delimiter) {
      const [mark, type] = delimiter;
      const after = text[index + mark.length];
      // Emphasis opens against a word, and `_` never inside one (snake_case).
      const opens =
        after !== undefined &&
        !/\s/.test(after) &&
        !(mark[0] === "_" && /\w/.test(text[index - 1] ?? ""));
      const close = opens ? findClose(text, mark, index + mark.length) : -1;
      if (close > 0) {
        flush();
        out.push({ type, children: parseInline(text.slice(index + mark.length, close), depth + 1, inLink) });
        index = close + mark.length;
        continue;
      }
    }
    plain += char;
    index += 1;
  }
  flush();
  return out;
}

function findClose(text: string, mark: string, from: number): number {
  let index = text.indexOf(mark, from);
  while (index > 0 && index - from <= MAX_SPAN && scanBudget-- > 0) {
    const before = text[index - 1];
    const after = text[index + mark.length];
    const wordAfter = mark[0] === "_" && after !== undefined && /\w/.test(after);
    // `**` is not a closing `*`.
    const doubled = mark.length === 1 && (after === mark || text[index - 1] === mark);
    if (!/\s/.test(before ?? "") && !wordAfter && !doubled) return index;
    index = text.indexOf(mark, index + mark.length);
  }
  return -1;
}

/** A message's words without its markdown, for a one-line preview. */
export function plainText(source: string): string {
  const inline = (parts: Inline[]): string =>
    parts.map((part) => ("children" in part ? inline(part.children) : part.text)).join("");
  const block = (item: MarkdownBlock): string => {
    switch (item.type) {
      case "paragraph":
      case "heading":
        return inline(item.inline);
      case "code":
        return item.text;
      case "list":
        return item.items.map((entry) => entry.map(block).join(" ")).join(" ");
      case "quote":
        return item.children.map(block).join(" ");
      case "table":
        return item.rows.map((row) => row.map(inline).join(" ")).join(" ");
      case "rule":
        return "";
    }
  };
  return parseMarkdown(source).map(block).join(" ").replace(/\s+/g, " ").trim();
}
