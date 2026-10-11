/**
 * The plain-text twin of the quoted original. mail_outbox keeps only
 * `quoted_html`, so a reply's text part is built from the replied-to message's
 * stored `body_text` (already parsed text, so nothing here flattens HTML).
 */

/** `attribution` is the "On <date>, <name> wrote:" line, without a trailing newline. */
export function buildTextQuote(
  bodyText: string | null | undefined,
  attribution: string,
): string {
  const lines = (bodyText ?? "")
    .replace(/\r\n?/g, "\n")
    .replace(/\s+$/, "")
    .split("\n")
    .map((line) => (line === "" ? ">" : `> ${line}`));
  return `${attribution}\n${lines.join("\n")}`;
}
