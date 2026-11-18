// Parsing of the "Paste a pairing link" field (F-02). Owner: WS-M (W-04).
// Accepts a pasted `/pair/:code#fp=...` URL of this same origin, or a bare code. Anything else is
// rejected: a link to another origin is never followed.
import { USER_CODE_ALPHABET, USER_CODE_LENGTH } from "@monoptah/protocol";

export interface PairingTarget {
  /** Normalized user code (upper case). */
  code: string;
  /** The raw `fp` value of the link, kept so the pair page can compare it; undefined for a bare code. */
  fp?: string;
}

/** Upper-cases and strips spaces and hyphens; returns null unless it is a well-formed user code. */
export function normalizePairingCode(input: string): string | null {
  const code = input.replace(/[\s-]/g, "").toUpperCase();
  if (code.length !== USER_CODE_LENGTH) return null;
  for (const char of code) if (!USER_CODE_ALPHABET.includes(char)) return null;
  return code;
}

/** `origin` is injectable for tests; it defaults to this page's origin. */
export function parsePairingInput(input: string, origin: string = window.location.origin): PairingTarget | null {
  const text = input.trim();
  if (text === "") return null;
  const bare = normalizePairingCode(text);
  if (bare) return { code: bare };

  let url: URL;
  try {
    url = new URL(text);
  } catch {
    return null;
  }
  if (url.origin !== origin) return null;
  const match = /^\/pair\/([^/]+)\/?$/.exec(url.pathname);
  if (!match) return null;
  let segment: string;
  try {
    segment = decodeURIComponent(match[1] ?? "");
  } catch {
    return null;
  }
  const code = normalizePairingCode(segment);
  if (!code) return null;
  const fp = new URLSearchParams(url.hash.replace(/^#/, "")).get("fp");
  return fp ? { code, fp } : { code };
}
