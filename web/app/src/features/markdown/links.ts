// Link allowlist of security.md section 8: https, http (shown with a warning), mailto and
// in-app /d/... paths. Everything else is plain text. The href is never decoded or
// rewritten before the check, and anything with whitespace, control, format or non-ASCII
// characters in it is refused outright, so scheme obfuscations cannot get through.
import { resolveFileReference } from "./fileLinks";

export type LinkTarget =
  | { kind: "external"; href: string; insecure: boolean }
  | { kind: "mailto"; href: string }
  | { kind: "internal"; href: string }
  | { kind: "file"; path: string; line?: number };

// ASCII graphic characters only: no space, no control, no Unicode (format, bidi, lookalikes).
const SAFE_CHARS = /^[\x21-\x7e]+$/;
const MAX_HREF = 2000;

export function classifyLink(rawHref: string): LinkTarget | null {
  if (rawHref.length === 0 || rawHref.length > MAX_HREF) return null;
  const href = rawHref;
  if (!SAFE_CHARS.test(href) || href.includes("\\")) return null;
  // Protocol-relative and scheme-less network paths.
  if (href.startsWith("//")) return null;

  if (href.startsWith("/")) {
    if (href.startsWith("/d/") && !href.includes("..") && !href.includes("//")) {
      return { kind: "internal", href };
    }
    // An absolute file path such as /home/me/app.ts:12 is a file link, never a navigation.
    const file = resolveFileReference(href);
    return file ? { kind: "file", ...file } : null;
  }

  const scheme = /^([a-z][a-z0-9+.-]*):/i.exec(href);
  if (!scheme || /^[^:/]+:\d+(?::\d+)?$/.test(href)) {
    // No scheme: a workspace-relative file reference or nothing.
    const file = resolveFileReference(href);
    return file ? { kind: "file", ...file } : null;
  }
  const name = (scheme[1] ?? "").toLowerCase();
  if (name === "file") {
    const file = resolveFileReference(href);
    return file ? { kind: "file", ...file } : null;
  }
  if (name === "https" || name === "http") {
    if (!/^https?:\/\/[^/?#]+/i.test(href)) return null;
    let url: URL;
    try {
      url = new URL(href);
    } catch {
      return null;
    }
    if ((url.protocol !== "https:" && url.protocol !== "http:") || !url.hostname) return null;
    if (url.username || url.password) return null;
    return { kind: "external", href: url.href, insecure: url.protocol === "http:" };
  }
  if (name === "mailto") {
    if (!/^mailto:[^/\s?#]+/i.test(href)) return null;
    try {
      const url = new URL(href);
      if (url.protocol !== "mailto:") return null;
      return { kind: "mailto", href: url.href };
    } catch {
      return null;
    }
  }
  return null;
}
