// Text that a desktop (or a person who typed it) chose: machine names and OS strings. It is rendered as
// plain React text, never as markup; this only removes what can make a name look like another one.

/** Control characters and invisible format characters (bidi overrides, zero width, line separators). */
const HIDDEN = /[\p{Cc}\p{Cf}\p{Zl}\p{Zp}]/gu;

/** Removes hidden characters, collapses whitespace and cuts to `max` characters with an ellipsis. */
export function safeLabel(text: string, max = 64, fallback = "Unnamed"): string {
  const clean = text.replace(/\s+/g, " ").replace(HIDDEN, "").trim();
  if (clean === "") return fallback;
  const chars = [...clean];
  return chars.length > max ? `${chars.slice(0, max - 1).join("")}…` : clean;
}
