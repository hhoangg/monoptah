// Small crypto helpers shared by the accounts modules (A-03 passkeys, A-04 sessions, A-05 recovery,
// A-06 invites, A-07 rate limits and audit). Web Crypto only, no dependency.

const encoder = new TextEncoder();

/** Lowercase hex of a byte array. */
export function toHex(bytes: Uint8Array): string {
  let out = "";
  for (const byte of bytes) out += byte.toString(16).padStart(2, "0");
  return out;
}

/** Unpadded base64url of a byte array. */
export function toBase64Url(bytes: Uint8Array): string {
  let binary = "";
  for (const byte of bytes) binary += String.fromCharCode(byte);
  return btoa(binary).replaceAll("+", "-").replaceAll("/", "_").replace(/=+$/, "");
}

/** `bytes` random bytes from the CSPRNG, as unpadded base64url (32 bytes give a 256-bit token of 43 chars). */
export function randomToken(bytes = 32): string {
  return toBase64Url(crypto.getRandomValues(new Uint8Array(bytes)));
}

/** SHA-256 of a UTF-8 string, as lowercase hex. Used for high-entropy secrets (session tokens, device codes). */
export async function sha256Hex(data: string): Promise<string> {
  return toHex(new Uint8Array(await crypto.subtle.digest("SHA-256", encoder.encode(data))));
}

/** HMAC-SHA-256 of `data` under the UTF-8 `key`, as lowercase hex (pepper and salt based hashes). */
export async function hmacSha256Hex(key: string, data: string): Promise<string> {
  const cryptoKey = await crypto.subtle.importKey("raw", encoder.encode(key), { name: "HMAC", hash: "SHA-256" }, false, ["sign"]);
  return toHex(new Uint8Array(await crypto.subtle.sign("HMAC", cryptoKey, encoder.encode(data))));
}
