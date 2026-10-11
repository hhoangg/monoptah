/**
 * `cid:` handling shared by the reader (resolving received mail) and the
 * composer (checking what we send). Pure string and HTML functions.
 */

/** `url(...)` in CSS; global, so only use it with `matchAll` or `replace`. */
export const CSS_URL = /url\(\s*(['"]?)(.*?)\1\s*\)/gi;

const parse = (html: string) =>
  new DOMParser().parseFromString(html, "text/html");

/** Matches a `cid:` reference to `Content-ID`, which is stored without `<>`. */
export function cidKey(raw: string): string {
  let value = raw.trim().replace(/^<|>$/g, "");
  try {
    value = decodeURIComponent(value);
  } catch {
    // keep the undecoded form
  }
  return value.toLowerCase();
}

/** `cid:` references appear on `img src` and inside `style` url(...). */
export function cidReferences(html: string): string[] {
  const keys = new Set<string>();
  const doc = parse(html);
  for (const img of doc.querySelectorAll("img[src]")) {
    const src = img.getAttribute("src") ?? "";
    if (/^cid:/i.test(src)) keys.add(cidKey(src.slice(4)));
  }
  for (const el of doc.querySelectorAll("[style]")) {
    for (const m of (el.getAttribute("style") ?? "").matchAll(CSS_URL)) {
      if (/^cid:/i.test(m[2])) keys.add(cidKey(m[2].slice(4)));
    }
  }
  return [...keys];
}
