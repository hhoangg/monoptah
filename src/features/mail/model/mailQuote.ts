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

const ABSOLUTE_HTTP = /^https?:\/\//i;

/**
 * The original's HTML as it should leave in a reply or forward.
 *
 * Rust hands the reader `src=""` plus `data-mono-src` for every remote image
 * so nothing loads before the user agrees. That is a reader-side state, not
 * part of the message: sent as it is, every logo and signature arrives blank
 * with an internal attribute. So the URL goes back to `src`. Sending a link is
 * not loading it, and the recipient of a reply is the sender of the original.
 * A forward does let the new recipient's client fetch it, as in any mail
 * client that quotes HTML; the user chose to forward.
 *
 * Only absolute http(s) URLs come back. A relative one has no base in the
 * recipient's client, so it is treated like the images below.
 *
 * `cid:` images point at parts of the original that a reply or forward does not
 * attach, so they would arrive broken. They become their alt text, or nothing:
 * a broken-image box with a `cid:` URL tells the reader less than the words the
 * sender gave it, and an image with no alt text is decoration (a logo, a
 * spacer, a tracking pixel) whose absence costs the quote nothing.
 */
export function quotableHtml(html: string): string {
  // DOMParser documents are inert: nothing here loads or runs.
  const doc = new DOMParser().parseFromString(html, "text/html");
  for (const img of doc.querySelectorAll("img")) {
    const blocked = img.getAttribute("data-mono-src");
    img.removeAttribute("data-mono-src");
    if (blocked !== null && ABSOLUTE_HTTP.test(blocked)) {
      img.setAttribute("src", blocked);
      continue;
    }
    const src = img.getAttribute("src") ?? "";
    if (/^data:image\//i.test(src) || ABSOLUTE_HTTP.test(src)) continue;
    // cid:, a relative reference, or an emptied one: nothing the recipient can load.
    const alt = (img.getAttribute("alt") ?? "").trim();
    img.replaceWith(doc.createTextNode(alt));
  }
  return doc.body.innerHTML;
}
