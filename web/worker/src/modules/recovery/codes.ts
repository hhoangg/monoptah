// Recovery codes (security.md 3.2): 16 characters of Crockford base32 (80 bits) shown as `XXXX-XXXX-XXXX-XXXX`,
// stored only as HMAC-SHA-256 under the server pepper (`RECOVERY_PEPPER`). A-03 issues the first batch at
// registration; A-05 uses and regenerates them. A code is never logged and never stored in clear.
import type { Env } from "../../env.ts";
import { hmacSha256Hex } from "../../lib/crypto.ts";

/** Crockford base32: digits and letters without I, L, O and U. */
export const CROCKFORD_ALPHABET = "0123456789ABCDEFGHJKMNPQRSTVWXYZ";
export const RECOVERY_CODES_PER_BATCH = 10;
export const RECOVERY_CODE_LENGTH = 16;

/** One random code in display form (`XXXX-XXXX-XXXX-XXXX`, uppercase). 256 is a multiple of 32, so `& 31` has no bias. */
export function generateRecoveryCode(): string {
  const bytes = crypto.getRandomValues(new Uint8Array(RECOVERY_CODE_LENGTH));
  let raw = "";
  for (const byte of bytes) raw += CROCKFORD_ALPHABET[byte & 31];
  return `${raw.slice(0, 4)}-${raw.slice(4, 8)}-${raw.slice(8, 12)}-${raw.slice(12)}`;
}

/** Uppercase, no hyphens or spaces, and the Crockford aliases: I and L read as 1, O reads as 0. */
export function normalizeRecoveryCode(code: string): string {
  return code.toUpperCase().replace(/[\s-]/g, "").replaceAll("I", "1").replaceAll("L", "1").replaceAll("O", "0");
}

/** `recovery_codes.code_hash`: HMAC-SHA-256 hex over the normalized code, keyed by the pepper. */
export function hashRecoveryCode(env: Pick<Env, "RECOVERY_PEPPER">, code: string): Promise<string> {
  return hmacSha256Hex(env.RECOVERY_PEPPER, normalizeRecoveryCode(code));
}

export interface NewRecoveryCodes {
  /** The codes to show the user once. */
  codes: string[];
  /** The matching hashes, in the same order. */
  hashes: string[];
}

/** A fresh batch of 10 codes and their hashes. */
export async function generateRecoveryCodes(env: Pick<Env, "RECOVERY_PEPPER">): Promise<NewRecoveryCodes> {
  const codes = Array.from({ length: RECOVERY_CODES_PER_BATCH }, generateRecoveryCode);
  const hashes = await Promise.all(codes.map((code) => hashRecoveryCode(env, code)));
  return { codes, hashes };
}
