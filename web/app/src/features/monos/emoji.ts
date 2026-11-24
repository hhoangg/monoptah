// The `:emo[<emotion>]` token a Mono writes to ask for a picture. The desktop strips it from the text it sends
// and sends the resolved pictures next to it, so the web normally never sees one. This is a port of the
// token grammar of `crates/monocode-core/src/mono/emoji/token.rs` for the places that can still meet raw
// text (a Mono's last message in a list, an older desktop): a token is `:emo[` + a name up to the first `]`,
// a `[` or a line break ends the search, and nothing inside a fenced block or an inline code span counts.
// Offsets are UTF-16 indices; the only length that is counted in UTF-8 bytes is the one the core counts so.

const PREFIX = ":emo[";
/** An emotion name has at most this many characters (`MAX_EMOTION_NAME` of the core). */
const MAX_NAME_CHARS = 32;
/** The most UTF-8 bytes between the brackets: 4 bytes per character plus trimmed padding. */
const MAX_NAME = MAX_NAME_CHARS * 4 + 32;

interface Range {
  start: number;
  end: number;
}

interface Token extends Range {
  name: string;
}

const utf8Length = (codePoint: number): number => (codePoint < 0x80 ? 1 : codePoint < 0x800 ? 2 : codePoint < 0x10000 ? 3 : 4);

function utf8Size(text: string): number {
  let size = 0;
  for (const char of text) size += utf8Length(char.codePointAt(0) ?? 0);
  return size;
}

/** The tokens of a plain text run (the caller passes runs that are not code). */
function findTokens(text: string): Token[] {
  const tokens: Token[] = [];
  let at = 0;
  for (;;) {
    const start = text.indexOf(PREFIX, at);
    if (start < 0) break;
    const body = start + PREFIX.length;
    let end = -1;
    let offset = 0;
    for (let i = body; i < text.length; ) {
      const point = text.codePointAt(i) ?? 0;
      const char = String.fromCodePoint(point);
      if (char === "]") {
        end = i;
        break;
      }
      if (char === "[" || char === "\n" || char === "\r") break;
      if (offset >= MAX_NAME) break;
      offset += utf8Length(point);
      i += char.length;
    }
    const name = end >= 0 ? text.slice(body, end).trim() : null;
    if (end >= 0 && name !== null && [...name].length <= MAX_NAME_CHARS) {
      tokens.push({ start, end: end + 1, name });
      at = end + 1;
    } else {
      at = start + 1;
    }
  }
  return tokens;
}

const trimStartSpaces = (line: string): string => line.replace(/^ +/, "");

/** The marker, width and indent of a line that looks like a code fence (``` or ~~~), at any indent. */
function fenceLine(line: string): { marker: string; width: number; indent: number } | null {
  const trimmed = trimStartSpaces(line);
  const marker = trimmed[0] ?? "";
  if (marker !== "`" && marker !== "~") return null;
  let width = 0;
  while (trimmed[width] === marker) width++;
  return width >= 3 ? { marker, width, indent: line.length - trimmed.length } : null;
}

/** Whether one of `lines` closes a fence opened with `marker` and `width` at `indent`. */
function closesLater(lines: string[], marker: string, width: number, indent: number): boolean {
  return lines.some((line) => {
    const fence = fenceLine(line);
    return !!fence && fence.marker === marker && fence.width >= width && fence.indent <= indent && trimStartSpaces(line).slice(fence.width).trim() === "";
  });
}

function runLength(text: string, from: number, to: number): number {
  let n = 0;
  while (from + n < to && text[from + n] === "`") n++;
  return n;
}

function inlineSpans(text: string, spanStart: number, spanEnd: number, openEnded: boolean, out: Range[]): void {
  let i = spanStart;
  while (i < spanEnd) {
    const char = text[i];
    if (char === "\\" && text[i + 1] === "`") {
      i += 2;
    } else if (char === "`") {
      const run = runLength(text, i, spanEnd);
      let j = i + run;
      let closed = -1;
      while (j < spanEnd) {
        if (text[j] === "`") {
          const width = runLength(text, j, spanEnd);
          if (width === run) {
            closed = j + width;
            break;
          }
          j += width;
        } else {
          j += 1;
        }
      }
      if (closed >= 0) {
        out.push({ start: i, end: closed });
        i = closed;
      } else if (openEnded) {
        out.push({ start: i, end: spanEnd });
        return;
      } else {
        i += run;
      }
    } else {
      i += 1;
    }
  }
}

/**
 * Ranges of markdown code: fenced blocks and inline code spans. With `openEnded`, a backtick run with no
 * closing run counts as code up to the end of its paragraph (what a streaming view needs).
 */
function codeRanges(text: string, openEnded: boolean): Range[] {
  const ranges: Range[] = [];
  let fence: { marker: string; width: number; from: number; indent: number } | null = null;
  let paragraph = 0;
  let offset = 0;
  const lines = text.match(/[^\n]*\n|[^\n]+/g) ?? [];
  for (let at = 0; at < lines.length; at++) {
    const line = lines[at] ?? "";
    const start = offset;
    offset += line.length;
    const found = fenceLine(line);
    if (!found) {
      if (!fence && line.trim() === "") {
        inlineSpans(text, paragraph, start, openEnded, ranges);
        paragraph = offset;
      }
      continue;
    }
    if (fence) {
      const rest = trimStartSpaces(line).slice(found.width).trim();
      if (found.indent <= Math.max(fence.indent, 3) && found.marker === fence.marker && found.width >= fence.width && rest === "") {
        ranges.push({ start: fence.from, end: offset });
        fence = null;
        paragraph = offset;
      }
    } else if (found.indent <= 3 || closesLater(lines.slice(at + 1), found.marker, found.width, found.indent)) {
      // A fence indented past 3 spaces is code in markdown, unless it sits in a list item: then a closer
      // with the same or less indent follows.
      inlineSpans(text, paragraph, start, openEnded, ranges);
      fence = { marker: found.marker, width: found.width, from: start, indent: found.indent };
    }
  }
  if (fence) ranges.push({ start: fence.from, end: text.length });
  else inlineSpans(text, paragraph, text.length, openEnded, ranges);
  return ranges;
}

const inCode = (ranges: Range[], at: number): boolean => ranges.some((range) => at >= range.start && at < range.end);

/** Whether a line is nothing but quote markers and a list marker, such as `-` or `> 1.`. */
function onlyBlockPrefix(line: string): boolean {
  let rest = line.trim();
  while (rest.startsWith(">")) rest = rest.slice(1).trimStart();
  if (rest === "" || rest === "-" || rest === "*" || rest === "+") return true;
  return /^\d+[.)]$/.test(rest);
}

/** Removes the tokens from raw markdown, leaving code untouched. A line that held only tokens goes with its line break. */
function strip(markdown: string): string {
  if (!markdown.includes(PREFIX)) return markdown;
  const code = codeRanges(markdown, false);
  const tokens = findTokens(markdown).filter((token) => !inCode(code, token.start));
  if (tokens.length === 0) return markdown;
  let out = "";
  let offset = 0;
  for (const line of markdown.match(/[^\n]*\n|[^\n]+/g) ?? []) {
    const start = offset;
    offset += line.length;
    const here = tokens.filter((token) => token.start >= start && token.end <= offset);
    if (here.length === 0) {
      out += line;
      continue;
    }
    const body = line.replace(/[\n\r]+$/, "");
    const ending = line.slice(body.length);
    let kept = "";
    let at = 0;
    for (const token of here) {
      kept += body.slice(at, token.start - start);
      at = token.end - start;
      if (kept.endsWith(" ")) {
        if (body.slice(at).startsWith(" ")) at += 1;
        else if (at === body.length) kept = kept.slice(0, -1);
      }
    }
    kept += body.slice(at);
    if (kept.trim() !== "" && !onlyBlockPrefix(kept)) out += kept + ending;
  }
  return out;
}

/** Where an unfinished token at the very end of `text` starts (`:`, `:em`, `:emo[ha`), or null. */
function incompleteTail(text: string): number | null {
  const window = Math.max(0, text.length - (PREFIX.length + MAX_NAME + 1));
  for (let start = text.indexOf(":", window); start >= 0; start = text.indexOf(":", start + 1)) {
    const tail = text.slice(start);
    const open = tail.startsWith(PREFIX) ? (() => {
      const rest = tail.slice(PREFIX.length);
      return utf8Size(rest) <= MAX_NAME && !/[\][\n\r]/.test(rest);
    })() : PREFIX.startsWith(tail);
    if (!open) continue;
    return inCode(codeRanges(text, true), start) ? null : start;
  }
  return null;
}

/**
 * `text` without its `:emo[...]` tokens and without an unfinished token at its end. For a Mono's text that
 * has no resolved pictures (a preview, a reply from an older desktop): the raw token must never show.
 * `trimTail: false` keeps a trailing `:` or `:em` (a finished reply that is not streaming any more).
 */
export function stripEmoTokens(text: string, { trimTail = true }: { trimTail?: boolean } = {}): string {
  if (!text.includes(":")) return text;
  const stripped = strip(text);
  if (!trimTail) return stripped;
  const tail = incompleteTail(stripped);
  return tail === null ? stripped : stripped.slice(0, tail);
}
