// Bytes and keys of the terminal: UTF-8 chunking for `terminal.write`, base64 for the wire and the keys the helper row sends.
// Pure functions, no DOM.

const encoder = new TextEncoder();

export function utf8(text: string): Uint8Array {
  return encoder.encode(text);
}

export function toBase64(bytes: Uint8Array): string {
  let binary = "";
  // Slices keep `String.fromCharCode` argument lists small.
  for (let at = 0; at < bytes.length; at += 8192) binary += String.fromCharCode(...bytes.subarray(at, at + 8192));
  return btoa(binary);
}

/** Decodes standard base64; null for text that is not base64 (a desktop that sent something else). */
export function fromBase64(text: string): Uint8Array | null {
  if (text === "") return new Uint8Array(0);
  try {
    return Uint8Array.from(atob(text), (char) => char.charCodeAt(0));
  } catch {
    return null;
  }
}

/**
 * The end (exclusive) of the first chunk of at most `max` bytes of `bytes` that does not cut a UTF-8 character in half.
 * A chunk is at least one character: a limit below the length of the first character still takes that character.
 */
export function chunkEnd(bytes: Uint8Array, max: number): number {
  if (bytes.length <= max) return bytes.length;
  let end = max;
  // A continuation byte is 10xxxxxx: step back to the byte that starts the character.
  while (end > 0 && ((bytes[end] ?? 0) & 0xc0) === 0x80) end -= 1;
  return end === 0 ? Math.min(bytes.length, max) : end;
}

// --- Keys of the helper row ---

export type Arrow = "up" | "down" | "right" | "left";
const ARROW_FINAL: Readonly<Record<Arrow, string>> = { up: "A", down: "B", right: "C", left: "D" };

/** The sequence of an arrow key: `ESC [ A` normally, `ESC O A` while the program asked for application cursor keys. */
export function arrowSequence(arrow: Arrow, applicationCursor: boolean): string {
  return `\x1b${applicationCursor ? "O" : "["}${ARROW_FINAL[arrow]}`;
}

export const KEY_ESC = "\x1b";
export const KEY_TAB = "\t";
/** Symbols that are hard to reach on a phone keyboard. */
export const SYMBOL_KEYS = ["|", "/", "-", "~", "$"] as const;

/** The control character of a typed key (`c` gives 0x03), or null when the key has none. */
export function controlCharacter(key: string): string | null {
  if (key.length !== 1) return null;
  const code = key.charCodeAt(0);
  if ((code >= 0x61 && code <= 0x7a) || (code >= 0x41 && code <= 0x5a)) return String.fromCharCode(code & 0x1f);
  switch (key) {
    case "@":
    case " ":
      return "\x00";
    case "[":
      return "\x1b";
    case "\\":
      return "\x1c";
    case "]":
      return "\x1d";
    case "^":
      return "\x1e";
    case "_":
      return "\x1f";
    case "?":
      return "\x7f";
    default:
      return null;
  }
}

/** Sticky modifiers of the helper row: each applies to the next typed key only. */
export interface Modifiers {
  ctrl: boolean;
  alt: boolean;
}

/**
 * What goes to the shell for `data` typed (or sent by the row) while `modifiers` are on. Ctrl turns one letter into its
 * control character, Alt prefixes ESC; anything longer than one character (a paste, an escape sequence) passes unchanged.
 * The caller clears the modifiers after any key.
 */
export function applyModifiers(data: string, modifiers: Modifiers): string {
  if ((!modifiers.ctrl && !modifiers.alt) || [...data].length !== 1) return data;
  let out = data;
  if (modifiers.ctrl) out = controlCharacter(data) ?? data;
  return modifiers.alt ? `\x1b${out}` : out;
}
