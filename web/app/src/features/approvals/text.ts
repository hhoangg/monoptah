// What the approval card shows of untrusted provider text. It is never markup: this turns the text into
// segments of plain text and visible markers, so bidi controls, invisible characters and blank-line or space
// padding cannot hide part of a command from the person who approves it (security review I-3).

export interface TextSegment {
  text: string;
  /** True for a marker the card made up (`<U+202E>`, `[12 blank lines]`), shown in a distinct style. */
  marker?: boolean;
}

export interface ShownDetail {
  segments: TextSegment[];
  /** Lines of the shown text, after blank-line collapsing. */
  lines: number;
  /** True when the provider text was longer than `max` and was cut. */
  truncated: boolean;
  /** Length of the whole provider text in UTF-16 units. */
  total: number;
}

// Characters that draw nothing yet are not spaces to a reader: Unicode space separators (NBSP, ideographic
// space, the U+2000 block), tab, and the blank-glyph characters (Hangul fillers U+115F, U+1160, U+3164, U+FFA0
// and the Braille blank U+2800). A line made of only these counts as blank, so padding cannot push a command
// out of view (security re-review N-1).
const BLANK = String.raw`[\p{Zs}\t\u115F\u1160\u3164\uFFA0\u2800]`;
// Default-ignorable marks that render as nothing: combining grapheme joiner, Khmer inherent vowels,
// Mongolian free variation selectors, variation selectors and their supplement. Written as alternatives
// (not one character class) because they are combining characters.
const IGNORABLE = String.raw`\u034F|\u17B4|\u17B5|\u180B|\u180C|\u180D|\u180F|[\uFE00-\uFE0F]|\u{E0100}|[\u{E0101}-\u{E01EF}]`;
// 3+ blank lines in a row, 40+ blank characters in a row, then any control, format, separator or invisible
// character except newline, tab and the plain space.
const HIDING = new RegExp(
  `(\\n${BLANK}*(?:\\n${BLANK}*){3,})|(${BLANK}{40,})|((?![\\n\\t ])(?:[\\p{Cc}\\p{Cf}\\p{Zl}\\p{Zp}\\p{Zs}\\u115F\\u1160\\u3164\\uFFA0\\u2800]|${IGNORABLE}))`,
  "gu",
);

function codePointMarker(char: string): string {
  return `<U+${char.codePointAt(0)!.toString(16).toUpperCase().padStart(4, "0")}>`;
}

/** Cuts to `max` UTF-16 units without splitting a surrogate pair. */
function cut(text: string, max: number): string {
  if (text.length <= max) return text;
  let end = max;
  const last = text.charCodeAt(end - 1);
  if (last >= 0xd800 && last <= 0xdbff) end -= 1;
  return text.slice(0, end);
}

export function showDetail(raw: string, max: number): ShownDetail {
  // Length and truncation are measured on the normalized text, so CRLF line ends do not count as cut text.
  const normalized = raw.replace(/\r\n/g, "\n");
  const total = normalized.length;
  const text = cut(normalized, max);
  const segments: TextSegment[] = [];
  let from = 0;
  const push = (value: string, marker = false) => {
    if (value === "") return;
    const previous = segments[segments.length - 1];
    if (!marker && previous && !previous.marker) previous.text += value;
    else segments.push(marker ? { text: value, marker: true } : { text: value });
  };
  for (const match of text.matchAll(HIDING)) {
    push(text.slice(from, match.index));
    from = match.index + match[0].length;
    if (match[1] !== undefined) {
      push("\n");
      push(`[${match[1].split("\n").length - 2} blank lines]`, true);
      push("\n");
    } else if (match[2] !== undefined) {
      push(`[${match[2].length} spaces]`, true);
    } else {
      push(codePointMarker(match[3]!), true);
    }
  }
  push(text.slice(from));
  const lines = segments.reduce((count, segment) => count + (segment.marker ? 0 : segment.text.split("\n").length - 1), 1);
  return { segments, lines, truncated: total > max, total };
}

/** One visible line of text for a label: hidden characters become markers too, whitespace is collapsed. */
export function showLabel(raw: string, max: number): TextSegment[] {
  const shown = showDetail(raw.replace(/\s+/g, " ").trim(), max);
  const segments = shown.segments;
  if (shown.truncated) segments.push({ text: "...", marker: true });
  return segments.length > 0 ? segments : [{ text: "Approval needed" }];
}
