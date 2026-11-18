// Post-sign-in redirect targets and the `#fp` fragment hand-over (architecture.md 3.1).
//
// `/login?redirect=<path>` is attacker-controllable, so only same-origin relative paths are
// accepted. The `#fp=<hex>` fragment (desktop key fingerprint, read by W-03) is public data, but it
// must not travel in the `redirect` query, so it is parked in sessionStorage while the user signs in.

import { canonicalFingerprint, fingerprintFromHash } from "./pair/fingerprint";

const FP_STORAGE_KEY = "monoptah.fp";
const FP_HEX_64 = /^[0-9a-f]{64}$/;
/** What a present but malformed `fp` comes back as: it is not hex, so it can never equal a fingerprint and shows the mismatch block. */
export const MALFORMED_FP = "invalid";
/** Resolved against this base to prove a target stays on the current origin. */
const BASE = "https://app.invalid";

/** Returns `/path?query#hash` when `raw` is a safe same-origin relative path, else null. */
export function safeRedirectTarget(raw: unknown): string | null {
  if (typeof raw !== "string" || raw.length === 0 || raw.length > 2048) return null;
  if (!raw.startsWith("/") || raw.startsWith("//") || raw.startsWith("/\\")) return null;
  // Control characters, tabs and backslashes are normalised by URL parsers in ways that can turn a
  // relative path into an absolute one.
  for (const char of raw) {
    const code = char.charCodeAt(0);
    if (code <= 0x1f || code === 0x7f || char === "\\") return null;
  }
  let url: URL;
  try {
    url = new URL(raw, BASE);
  } catch {
    return null;
  }
  if (url.origin !== BASE) return null;
  // Dot segments collapse during parsing: "/..//evil.example" becomes "//evil.example", which a
  // consumer that does not normalise again would read as a protocol-relative URL.
  if (url.pathname.startsWith("//")) return null;
  return `${url.pathname}${url.search}${url.hash}`;
}

/** The path the guard sends signed-out users to, with the page they wanted in `redirect`. */
export function loginRedirectTarget(pathname: string, searchStr: string): string {
  return safeRedirectTarget(`${pathname}${searchStr}`) ?? "/";
}

/**
 * Parks the `fp` of the hash (with the path it belongs to) before a redirect to /login. A valid value is saved
 * as lowercase hex. A present but malformed value (not 64 hex after the same clean-up the page does) is saved
 * only as a flag, never as the raw text, so the hard mismatch block survives the sign-in instead of
 * downgrading to "missing" (security review 2, L1).
 */
export function saveFingerprintFragment(pathname: string, hash: string): void {
  const raw = fingerprintFromHash(hash);
  if (raw === null) return;
  const canonical = canonicalFingerprint(raw);
  const entry = FP_HEX_64.test(canonical) ? { path: pathname, fp: canonical } : { path: pathname, malformed: true };
  try {
    sessionStorage.setItem(FP_STORAGE_KEY, JSON.stringify(entry));
  } catch {
    // Private mode or quota: the page then shows its "key cannot be checked" warning.
  }
}

/** Returns `#fp=<value>` once when it was saved for `pathname`, and forgets it. */
export function restoreFingerprintFragment(pathname: string): string | null {
  try {
    const raw = sessionStorage.getItem(FP_STORAGE_KEY);
    if (!raw) return null;
    const saved = JSON.parse(raw) as { path?: unknown; fp?: unknown; malformed?: unknown };
    if (saved.path !== pathname) return null;
    if (saved.malformed === true) {
      sessionStorage.removeItem(FP_STORAGE_KEY);
      return `#fp=${MALFORMED_FP}`;
    }
    if (typeof saved.fp !== "string" || !FP_HEX_64.test(saved.fp)) return null;
    sessionStorage.removeItem(FP_STORAGE_KEY);
    return `#fp=${saved.fp}`;
  } catch {
    return null;
  }
}

/** Where to go after a successful sign-in: the safe `redirect` (default `/`) plus a restored `#fp`. */
export function postSignInTarget(redirect: unknown): string {
  const target = safeRedirectTarget(redirect) ?? "/";
  const url = new URL(target, BASE);
  const fragment = url.hash ? null : restoreFingerprintFragment(url.pathname);
  return fragment ? `${url.pathname}${url.search}${fragment}` : target;
}
