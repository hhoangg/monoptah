// The `#fp=<hex>` comparison of the pairing page (architecture.md 6.4, security.md T3 and T5).
//
// The desktop shows a QR and a link that carry the full SHA-256 fingerprint of its public key in the URL
// fragment, which never reaches the server. The page compares it with the fingerprint the server returned
// for the same pairing code, so a host that substituted the desktop key is caught.
//
// The link client has a formatter of its own (`@/link/fingerprint`) but it is not part of the `@/link`
// public API, so the few lines needed here live in this folder.

export type FingerprintCheck = "match" | "mismatch" | "missing";

const HEX_64 = /^[0-9a-f]{64}$/;

/** Lowercase hex without whitespace, colons or dashes: the one form both sides are compared in. */
export function canonicalFingerprint(input: string): string {
  return input.replace(/[\s:-]/g, "").toLowerCase();
}

/** The raw `fp` value of a URL hash (`#fp=...`, with or without the `#`), or null when there is none. */
export function fingerprintFromHash(hash: string): string | null {
  const value = new URLSearchParams(hash.replace(/^#/, "")).get("fp");
  return value === null || value.trim() === "" ? null : value;
}

/**
 * `missing`: the link carried no fingerprint. `match`: the full 64 hex characters are equal. Anything else
 * that is present, including a truncated or malformed value, is a `mismatch`: a short prefix is not a proof.
 */
export function checkFingerprint(hash: string, serverFingerprint: string): FingerprintCheck {
  const fromLink = fingerprintFromHash(hash);
  if (fromLink === null) return "missing";
  const link = canonicalFingerprint(fromLink);
  const server = canonicalFingerprint(serverFingerprint);
  if (!HEX_64.test(link) || !HEX_64.test(server)) return "mismatch";
  // Both sides have the same public length, so a plain loop is enough (nothing secret is compared).
  let diff = 0;
  for (let i = 0; i < link.length; i += 1) diff |= link.charCodeAt(i) ^ server.charCodeAt(i);
  return diff === 0 ? "match" : "mismatch";
}

/** The first 32 hex characters as 8 groups of 4 (security.md 5), in two lines of 4 groups. */
export function fingerprintLines(fingerprint: string): [string, string] {
  const groups = canonicalFingerprint(fingerprint).slice(0, 32).match(/.{1,4}/g) ?? [];
  return [groups.slice(0, 4).join(" "), groups.slice(4, 8).join(" ")];
}
