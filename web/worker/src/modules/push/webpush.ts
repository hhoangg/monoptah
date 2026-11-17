// Web Push sender: RFC 8291 message encryption (aes128gcm, RFC 8188) and the push service request. WebCrypto only.
import { fromBase64Url } from "./vapid.ts";

export const RECORD_SIZE = 4096;
const encoder = new TextEncoder();

export interface EphemeralKey {
  /** ECDH P-256 private key. */
  privateKey: CryptoKey;
  /** 65-byte uncompressed point. */
  publicKey: Uint8Array;
}

export interface EncryptOptions {
  /** 16 random bytes by default; injectable for the RFC 8291 Appendix A vector. */
  salt?: Uint8Array;
  /** Fresh key pair by default; injectable for the vector. */
  ephemeral?: EphemeralKey;
}

function concat(...parts: Uint8Array[]): Uint8Array<ArrayBuffer> {
  const out = new Uint8Array(parts.reduce((n, p) => n + p.length, 0));
  let offset = 0;
  for (const part of parts) {
    out.set(part, offset);
    offset += part.length;
  }
  return out;
}

async function hkdf(ikm: Uint8Array, salt: Uint8Array, info: Uint8Array, bytes: number): Promise<Uint8Array<ArrayBuffer>> {
  const key = await crypto.subtle.importKey("raw", ikm as Uint8Array<ArrayBuffer>, "HKDF", false, ["deriveBits"]);
  return new Uint8Array(await crypto.subtle.deriveBits({ name: "HKDF", hash: "SHA-256", salt: salt as Uint8Array<ArrayBuffer>, info: info as Uint8Array<ArrayBuffer> }, key, bytes * 8));
}

async function newEphemeral(): Promise<EphemeralKey> {
  const pair = (await crypto.subtle.generateKey({ name: "ECDH", namedCurve: "P-256" }, false, ["deriveBits"])) as CryptoKeyPair;
  return { privateKey: pair.privateKey, publicKey: new Uint8Array((await crypto.subtle.exportKey("raw", pair.publicKey)) as ArrayBuffer) };
}

/**
 * Encrypts `plaintext` for a subscription (`p256dh`: 65-byte point, `auth`: 16-byte secret) into one aes128gcm
 * record: header (salt, rs, idlen, sender public key) then the ciphertext of `plaintext || 0x02`.
 */
export async function encryptPayload(plaintext: Uint8Array, p256dh: Uint8Array<ArrayBuffer>, auth: Uint8Array<ArrayBuffer>, options: EncryptOptions = {}): Promise<Uint8Array<ArrayBuffer>> {
  if (p256dh.length !== 65 || auth.length !== 16) throw new Error("Invalid subscription keys");
  if (plaintext.length + 17 > RECORD_SIZE) throw new Error("Payload does not fit one record");
  const salt = options.salt ?? crypto.getRandomValues(new Uint8Array(16));
  const sender = options.ephemeral ?? (await newEphemeral());
  const receiver = await crypto.subtle.importKey("raw", p256dh, { name: "ECDH", namedCurve: "P-256" }, false, []);
  // The runtime property is `public`; workers-types names it `$public`.
  const ecdh = new Uint8Array(await crypto.subtle.deriveBits({ name: "ECDH", public: receiver } as unknown as SubtleCryptoDeriveKeyAlgorithm, sender.privateKey, 256));
  const keyInfo = concat(encoder.encode("WebPush: info\0"), p256dh, sender.publicKey);
  const ikm = await hkdf(ecdh, auth, keyInfo, 32);
  const saltBytes = new Uint8Array(salt);
  const cek = await hkdf(ikm, saltBytes, encoder.encode("Content-Encoding: aes128gcm\0"), 16);
  const nonce = await hkdf(ikm, saltBytes, encoder.encode("Content-Encoding: nonce\0"), 12);
  const key = await crypto.subtle.importKey("raw", cek, "AES-GCM", false, ["encrypt"]);
  const record = await crypto.subtle.encrypt({ name: "AES-GCM", iv: nonce }, key, concat(plaintext, new Uint8Array([2])));
  const header = new Uint8Array(21);
  header.set(saltBytes, 0);
  new DataView(header.buffer).setUint32(16, RECORD_SIZE);
  header[20] = sender.publicKey.length;
  return concat(header, sender.publicKey, new Uint8Array(record));
}

export interface PushTarget {
  endpoint: string;
  /** Base64url as stored. */
  p256dh: string;
  auth: string;
}

export interface PushHeaders {
  authorization: string;
  urgency: "high" | "normal";
  topic: string;
}

export type PushFetch = (input: string, init: RequestInit) => Promise<Response>;

/** What the push service answered, reduced to what the notify flow acts on. */
export type PushOutcome = { kind: "ok" } | { kind: "gone" } | { kind: "backoff"; retryAfterSeconds: number } | { kind: "failed" };

export const MAX_BACKOFF_SECONDS = 3600;
const DEFAULT_BACKOFF_SECONDS = 60;
export const SEND_TIMEOUT_MS = 10_000;

/** `Retry-After` as seconds (a number or an HTTP date), capped at 1 h. */
export function parseRetryAfter(value: string | null, nowMs: number): number {
  if (value === null) return DEFAULT_BACKOFF_SECONDS;
  const trimmed = value.trim();
  let seconds: number;
  if (/^\d+$/.test(trimmed)) seconds = Number(trimmed);
  else {
    const date = Date.parse(trimmed);
    if (Number.isNaN(date)) return DEFAULT_BACKOFF_SECONDS;
    seconds = Math.ceil((date - nowMs) / 1000);
  }
  return Math.min(Math.max(seconds, 1), MAX_BACKOFF_SECONDS);
}

/** Encrypts and posts one message. Never throws: a network failure is `failed`. */
export async function sendWebPush(
  fetchFn: PushFetch,
  target: PushTarget,
  plaintext: Uint8Array,
  headers: PushHeaders,
  nowMs: number,
  options: EncryptOptions = {},
): Promise<PushOutcome> {
  try {
    const body = await encryptPayload(plaintext, fromBase64Url(target.p256dh), fromBase64Url(target.auth), options);
    const res = await fetchFn(target.endpoint, {
      method: "POST",
      headers: {
        Authorization: headers.authorization,
        TTL: "86400",
        Urgency: headers.urgency,
        Topic: headers.topic,
        "Content-Encoding": "aes128gcm",
        "Content-Type": "application/octet-stream",
      },
      body,
      redirect: "manual",
      signal: AbortSignal.timeout(SEND_TIMEOUT_MS),
    });
    // The body is never read: release the connection.
    void res.body?.cancel().catch(() => {});
    if (res.status >= 200 && res.status < 300) return { kind: "ok" };
    if (res.status === 404 || res.status === 410) return { kind: "gone" };
    if (res.status === 429) return { kind: "backoff", retryAfterSeconds: parseRetryAfter(res.headers.get("Retry-After"), nowMs) };
    return { kind: "failed" };
  } catch {
    return { kind: "failed" };
  }
}
