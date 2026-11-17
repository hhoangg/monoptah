// X25519 public keys on the wire: unpadded base64url of exactly 32 bytes (architecture.md 4.3). The fingerprint
// is the lowercase hex SHA-256 of the raw bytes, the same as `fingerprint` in mock-desktop/src/noise.ts.
import { toBase64Url, toHex } from "../../lib/crypto.ts";

export const PUBLIC_KEY_BYTES = 32;

/** Decodes unpadded base64url, or null for any other text. Canonical only: the text must re-encode to itself. */
export function decodeBase64Url(text: string): Uint8Array | null {
  if (!/^[A-Za-z0-9_-]+$/.test(text) || text.length % 4 === 1) return null;
  let binary: string;
  try {
    binary = atob(text.replaceAll("-", "+").replaceAll("_", "/"));
  } catch {
    return null;
  }
  const bytes = Uint8Array.from(binary, (ch) => ch.charCodeAt(0));
  return toBase64Url(bytes) === text ? bytes : null;
}

/** The 32 key bytes of `text`, or null when it is not base64url of exactly 32 bytes. */
export function decodePublicKey(text: string): Uint8Array | null {
  const bytes = decodeBase64Url(text);
  return bytes !== null && bytes.length === PUBLIC_KEY_BYTES ? bytes : null;
}

/** Lowercase hex SHA-256 of raw key bytes (never of the base64 text). */
export async function fingerprintOf(key: Uint8Array): Promise<string> {
  return toHex(new Uint8Array(await crypto.subtle.digest("SHA-256", key as BufferSource)));
}

/** Fingerprint of a stored base64url key; "" when the stored text is not a valid key. */
export async function fingerprintOfStored(text: string): Promise<string> {
  const key = decodePublicKey(text);
  return key === null ? "" : fingerprintOf(key);
}
