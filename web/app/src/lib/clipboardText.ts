// Text that came from an agent can hold invisible characters: terminal control codes (ESC ends a bracketed
// paste) and bidi overrides that reorder what the reader sees. The copy buttons drop them so what is pasted
// is what was shown. Newline and tab stay.

/** Marks and overrides that change the display order of text: ALM, LRM, RLM, LRE..RLO, LRI..PDI. */
const BIDI = new Set<number>([0x061c, 0x200e, 0x200f, 0x202a, 0x202b, 0x202c, 0x202d, 0x202e, 0x2066, 0x2067, 0x2068, 0x2069]);

function isUnsafe(code: number): boolean {
  if (code === 0x09 || code === 0x0a) return false;
  return code < 0x20 || code === 0x7f || BIDI.has(code);
}

export function safeClipboardText(text: string): string {
  let out = "";
  for (let index = 0; index < text.length; index += 1) {
    if (!isUnsafe(text.charCodeAt(index))) out += text[index];
  }
  return out;
}
