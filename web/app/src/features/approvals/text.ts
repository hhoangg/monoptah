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

// 3+ blank lines in a row, 40+ spaces in a row, then any control, format or line separator character
// except newline and tab.
const HIDING = /(\n[ \t]*(?:\n[ \t]*){3,})|( {40,})|((?![\n\t])[\p{Cc}\p{Cf}\p{Zl}\p{Zp}])/gu;

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
  const total = raw.length;
  const text = cut(raw.replace(/\r\n/g, "\n"), max);
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
