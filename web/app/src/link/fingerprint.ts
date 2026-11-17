import { fingerprint } from "./noise.ts";

export { fingerprint };

/** Full-length fingerprint: lowercase hex SHA-256, 64 chars. */
export const FINGERPRINT_HEX_LENGTH = 64;
/** Display: the first 32 hex chars as 8 groups of 4 (security.md 5). */
export const FINGERPRINT_DISPLAY_CHARS = 32;

/** Lowercases and strips whitespace and group separators; returns null unless exactly 64 hex chars remain. */
export function normalizeFingerprint(input: string): string | null {
  const hex = input.replace(/[\s:-]/g, "").toLowerCase();
  return /^[0-9a-f]{64}$/.test(hex) ? hex : null;
}

/** `abcd 1234 ...` (8 groups of 4) for showing a fingerprint to a person. */
export function displayFingerprint(hex: string): string {
  const head = hex.toLowerCase().slice(0, FINGERPRINT_DISPLAY_CHARS);
  return head.match(/.{1,4}/g)?.join(" ") ?? "";
}
