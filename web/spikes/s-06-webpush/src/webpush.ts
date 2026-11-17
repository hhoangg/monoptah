// S-06: in-house Web Push sender over WebCrypto only (no npm push library). Runs unchanged in Node 22+ and in workerd.
//
//  - RFC 8291 message encryption (ECDH P-256, HKDF-SHA-256, AES-128-GCM) in the aes128gcm content coding of RFC 8188.
//  - RFC 8292 VAPID: an ES256 JWT in `Authorization: vapid t=<jwt>, k=<public key>`.
//  - RFC 8030 request headers: TTL (required), Urgency, Topic.
//
// Everything that is random in production (salt, ephemeral sender key pair, clock) is injectable so the RFC 8291
// Appendix A vector can be reproduced byte for byte. Sources are cited in FINDINGS.md.

const encoder = new TextEncoder();

/** RFC 8188 record size we advertise: 4096 is what the push services accept as the maximum message size (RFC 8030 5.4). */
export const RECORD_SIZE = 4096;
/** Header of an aes128gcm body: salt (16) + record size (4) + key id length (1) + sender public key (65). */
export const HEADER_LENGTH = 16 + 4 + 1 + 65;
const TAG_LENGTH = 16;
/** Largest plaintext (before the delimiter and padding) that keeps the whole body within 4096 octets. */
export const MAX_PLAINTEXT_BYTES = RECORD_SIZE - HEADER_LENGTH - TAG_LENGTH - 1;
/** RFC 8292 section 2: `exp` must be at most 24 hours ahead. */
export const MAX_VAPID_LIFETIME_SECONDS = 24 * 60 * 60;

type Bytes = Uint8Array<ArrayBuffer>;

export function b64uEncode(bytes: Uint8Array): string {
  let binary = "";
  for (const byte of bytes) binary += String.fromCharCode(byte);
  return btoa(binary).replaceAll("+", "-").replaceAll("/", "_").replace(/=+$/, "");
}

export function b64uDecode(text: string): Bytes {
  if (!/^[A-Za-z0-9_-]*$/.test(text)) throw new Error("not base64url");
  const padded = text.replaceAll("-", "+").replaceAll("_", "/").padEnd(Math.ceil(text.length / 4) * 4, "=");
  return Uint8Array.from(atob(padded), (c) => c.charCodeAt(0));
}

function concat(...parts: Uint8Array[]): Bytes {
  const out = new Uint8Array(parts.reduce((n, p) => n + p.length, 0));
  let offset = 0;
  for (const part of parts) {
    out.set(part, offset);
    offset += part.length;
  }
  return out;
}

/** Subscription as returned by `PushSubscription.toJSON()`. */
export interface PushSubscriptionJson {
  endpoint: string;
  expirationTime?: number | null;
  keys: { p256dh: string; auth: string };
}

// ---------------------------------------------------------------------------------------------------------------
// RFC 8291 encryption

export interface EncryptOptions {
  /** 16 random octets. Production: omit (fresh random). Tests: the RFC 8291 Appendix A salt. */
  salt?: Uint8Array;
  /** The ephemeral application server key pair (ECDH P-256). Production: omit (fresh). Tests: the RFC key pair. */
  senderKeyPair?: CryptoKeyPair;
  /** Zero octets appended after the 0x02 delimiter, to hide the message length (RFC 8291 section 4). Default 0. */
  padding?: number;
}

/** Imports a raw 65 octet uncompressed P-256 public key for ECDH, validating the point (WebCrypto rejects off-curve points). */
async function importUserAgentKey(uaPublic: Uint8Array): Promise<CryptoKey> {
  if (uaPublic.length !== 65 || uaPublic[0] !== 4) throw new Error("p256dh must be a 65 octet uncompressed P-256 point");
  return crypto.subtle.importKey("raw", uaPublic as Bytes, { name: "ECDH", namedCurve: "P-256" }, false, []);
}

async function hkdf(salt: Uint8Array, ikm: Uint8Array, info: Uint8Array, bytes: number): Promise<Bytes> {
  const key = await crypto.subtle.importKey("raw", ikm as Bytes, "HKDF", false, ["deriveBits"]);
  return new Uint8Array(await crypto.subtle.deriveBits({ name: "HKDF", hash: "SHA-256", salt: salt as Bytes, info: info as Bytes }, key, bytes * 8));
}

/**
 * Encrypts `plaintext` for one user agent and returns the whole request body (RFC 8188 header + one record).
 * `uaPublic` is `keys.p256dh` decoded (65 octets), `authSecret` is `keys.auth` decoded (16 octets).
 */
export async function encryptPayload(plaintext: Uint8Array, uaPublic: Uint8Array, authSecret: Uint8Array, options: EncryptOptions = {}): Promise<Bytes> {
  const padding = options.padding ?? 0;
  if (!Number.isInteger(padding) || padding < 0) throw new Error("padding must be a non-negative integer");
  if (plaintext.length + padding > MAX_PLAINTEXT_BYTES) throw new Error(`payload too large: at most ${MAX_PLAINTEXT_BYTES} octets with padding`);
  if (authSecret.length !== 16) throw new Error("auth secret must be 16 octets");
  const salt = options.salt ?? crypto.getRandomValues(new Uint8Array(16));
  if (salt.length !== 16) throw new Error("salt must be 16 octets");

  const sender = options.senderKeyPair ?? ((await crypto.subtle.generateKey({ name: "ECDH", namedCurve: "P-256" }, true, ["deriveBits"])) as CryptoKeyPair);
  const asPublic = new Uint8Array((await crypto.subtle.exportKey("raw", sender.publicKey)) as ArrayBuffer);
  const uaKey = await importUserAgentKey(uaPublic);

  const ecdhSecret = new Uint8Array(await crypto.subtle.deriveBits({ name: "ECDH", public: uaKey }, sender.privateKey, 256));
  // RFC 8291 3.4: IKM = HKDF(salt = auth_secret, ikm = ecdh_secret, info = "WebPush: info" || 0x00 || ua_public || as_public, L = 32).
  // WebCrypto HKDF is Extract-then-Expand, which is PRK_key = HMAC(auth_secret, ecdh_secret), IKM = HMAC(PRK_key, key_info || 0x01).
  const keyInfo = concat(encoder.encode("WebPush: info\0"), uaPublic, asPublic);
  const ikm = await hkdf(authSecret, ecdhSecret, keyInfo, 32);
  // RFC 8188 2.2 and 2.3.
  const cek = await hkdf(salt, ikm, encoder.encode("Content-Encoding: aes128gcm\0"), 16);
  const nonce = await hkdf(salt, ikm, encoder.encode("Content-Encoding: nonce\0"), 12);

  // One record, the last one: plaintext || 0x02 || zero padding (RFC 8291 4, RFC 8188 2).
  const record = concat(plaintext, new Uint8Array([0x02]), new Uint8Array(padding));
  const aesKey = await crypto.subtle.importKey("raw", cek, "AES-GCM", false, ["encrypt"]);
  const ciphertext = new Uint8Array(await crypto.subtle.encrypt({ name: "AES-GCM", iv: nonce, tagLength: 128 }, aesKey, record));

  const header = new Uint8Array(HEADER_LENGTH);
  header.set(salt, 0);
  new DataView(header.buffer).setUint32(16, RECORD_SIZE);
  header[20] = asPublic.length;
  header.set(asPublic, 21);
  return concat(header, ciphertext);
}

/** Imports an ECDH key pair from raw octets (used by tests with the RFC keys). `publicRaw` is 65 octets, `privateRaw` 32. */
export async function importEcKeyPair(publicRaw: Uint8Array, privateRaw: Uint8Array, algorithm: "ECDH" | "ECDSA"): Promise<CryptoKeyPair> {
  const jwk = { kty: "EC", crv: "P-256", x: b64uEncode(publicRaw.slice(1, 33)), y: b64uEncode(publicRaw.slice(33, 65)) };
  const usages = algorithm === "ECDH" ? ["deriveBits"] : ["sign"];
  const publicKey = await crypto.subtle.importKey("jwk", jwk, { name: algorithm, namedCurve: "P-256" }, true, []);
  const privateKey = await crypto.subtle.importKey("jwk", { ...jwk, d: b64uEncode(privateRaw) }, { name: algorithm, namedCurve: "P-256" }, true, usages as KeyUsage[]);
  return { publicKey, privateKey };
}

/** Test helper: decrypts an aes128gcm body as the user agent would (RFC 8291 3.4, user agent role). */
export async function decryptPayload(body: Uint8Array, uaKeyPair: CryptoKeyPair, authSecret: Uint8Array): Promise<Bytes> {
  if (body.length < HEADER_LENGTH + TAG_LENGTH + 1) throw new Error("body too short");
  const salt = body.slice(0, 16);
  const idLength = body[20] ?? 0;
  if (idLength !== 65) throw new Error("unexpected key id length");
  const asPublic = body.slice(21, 21 + idLength);
  const uaPublic = new Uint8Array((await crypto.subtle.exportKey("raw", uaKeyPair.publicKey)) as ArrayBuffer);
  const asKey = await crypto.subtle.importKey("raw", asPublic, { name: "ECDH", namedCurve: "P-256" }, false, []);
  const ecdhSecret = new Uint8Array(await crypto.subtle.deriveBits({ name: "ECDH", public: asKey }, uaKeyPair.privateKey, 256));
  const ikm = await hkdf(authSecret, ecdhSecret, concat(encoder.encode("WebPush: info\0"), uaPublic, asPublic), 32);
  const cek = await hkdf(salt, ikm, encoder.encode("Content-Encoding: aes128gcm\0"), 16);
  const nonce = await hkdf(salt, ikm, encoder.encode("Content-Encoding: nonce\0"), 12);
  const aesKey = await crypto.subtle.importKey("raw", cek, "AES-GCM", false, ["decrypt"]);
  const record = new Uint8Array(await crypto.subtle.decrypt({ name: "AES-GCM", iv: nonce, tagLength: 128 }, aesKey, body.slice(HEADER_LENGTH)));
  let end = record.length;
  while (end > 0 && record[end - 1] === 0) end -= 1;
  if (end === 0 || record[end - 1] !== 0x02) throw new Error("missing 0x02 padding delimiter");
  return record.slice(0, end - 1);
}

// ---------------------------------------------------------------------------------------------------------------
// RFC 8292 VAPID

export interface VapidClaims {
  /** Origin of the push service endpoint, for example `https://fcm.googleapis.com` (RFC 8292 2). */
  audience: string;
  /** `mailto:` or `https:` contact URI (RFC 8292 2.1). */
  subject: string;
  /** Seconds since the epoch. Default: now + 12 h. Must be at most 24 h after `nowSeconds`. */
  expiresAt?: number;
  /** Injectable clock (seconds since the epoch) for tests. */
  nowSeconds?: number;
}

/** The `aud` claim for an endpoint: scheme, host and port only. */
export function audienceOf(endpoint: string): string {
  const url = new URL(endpoint);
  if (url.protocol !== "https:") throw new Error("push endpoints are https");
  return url.origin;
}

/** Imports the VAPID key pair from the base64url forms stored in config: public (65 octets) and private `d` (32 octets). */
export async function importVapidKeys(publicKeyB64u: string, privateKeyB64u: string): Promise<CryptoKeyPair> {
  const publicRaw = b64uDecode(publicKeyB64u);
  const privateRaw = b64uDecode(privateKeyB64u);
  if (publicRaw.length !== 65 || publicRaw[0] !== 4) throw new Error("VAPID public key must be a 65 octet uncompressed P-256 point");
  if (privateRaw.length !== 32) throw new Error("VAPID private key must be 32 octets");
  return importEcKeyPair(publicRaw, privateRaw, "ECDSA");
}

/** Generates a throwaway VAPID key pair in the base64url form used by `importVapidKeys` (for tests and the device runbook). */
export async function generateVapidKeys(): Promise<{ publicKey: string; privateKey: string }> {
  const pair = (await crypto.subtle.generateKey({ name: "ECDSA", namedCurve: "P-256" }, true, ["sign", "verify"])) as CryptoKeyPair;
  const publicKey = b64uEncode(new Uint8Array((await crypto.subtle.exportKey("raw", pair.publicKey)) as ArrayBuffer));
  const jwk = (await crypto.subtle.exportKey("jwk", pair.privateKey)) as JsonWebKey;
  return { publicKey, privateKey: jwk.d ?? "" };
}

/** Builds and signs the VAPID JWT (ES256, header `{"typ":"JWT","alg":"ES256"}`). WebCrypto already returns JOSE `r || s`. */
export async function createVapidJwt(claims: VapidClaims, privateKey: CryptoKey): Promise<string> {
  const now = claims.nowSeconds ?? Math.floor(Date.now() / 1000);
  const exp = claims.expiresAt ?? now + 12 * 60 * 60;
  if (exp <= now) throw new Error("exp must be in the future");
  if (exp - now > MAX_VAPID_LIFETIME_SECONDS) throw new Error("exp must be at most 24 hours ahead (RFC 8292 2)");
  if (!/^(mailto:|https:\/\/)/.test(claims.subject)) throw new Error("sub must be a mailto: or https: URI");
  const aud = new URL(claims.audience);
  if (aud.origin !== claims.audience) throw new Error("aud must be an origin (scheme, host, port)");
  const signingInput = `${b64uEncode(encoder.encode(JSON.stringify({ typ: "JWT", alg: "ES256" })))}.${b64uEncode(
    encoder.encode(JSON.stringify({ aud: claims.audience, exp, sub: claims.subject })),
  )}`;
  const signature = new Uint8Array(await crypto.subtle.sign({ name: "ECDSA", hash: "SHA-256" }, privateKey, encoder.encode(signingInput)));
  return `${signingInput}.${b64uEncode(signature)}`;
}

/** `Authorization` header value of RFC 8292 3: `vapid t=<jwt>, k=<base64url public key>`. */
export function vapidAuthorization(jwt: string, publicKeyB64u: string): string {
  return `vapid t=${jwt}, k=${publicKeyB64u}`;
}

// ---------------------------------------------------------------------------------------------------------------
// RFC 8030 request building

export type Urgency = "very-low" | "low" | "normal" | "high";
const URGENCIES: readonly string[] = ["very-low", "low", "normal", "high"];

export interface PushRequestOptions {
  /** Seconds the push service stores the message while the device is offline. Required; 0 means deliver now or drop. */
  ttl: number;
  urgency?: Urgency;
  /** Replaces a queued message with the same topic. At most 32 characters of the base64url alphabet (RFC 8030 5.4). */
  topic?: string;
}

/** Validates and returns the RFC 8030 headers `TTL`, `Urgency`, `Topic`. */
export function pushHeaders(options: PushRequestOptions): Record<string, string> {
  if (!Number.isInteger(options.ttl) || options.ttl < 0) throw new Error("TTL is required and must be a non-negative integer");
  const headers: Record<string, string> = { TTL: String(options.ttl) };
  if (options.urgency !== undefined) {
    if (!URGENCIES.includes(options.urgency)) throw new Error(`Urgency must be one of ${URGENCIES.join(", ")}`);
    headers["Urgency"] = options.urgency;
  }
  if (options.topic !== undefined) {
    if (!/^[A-Za-z0-9_-]{1,32}$/.test(options.topic)) throw new Error("Topic must be 1 to 32 characters of the base64url alphabet");
    headers["Topic"] = options.topic;
  }
  return headers;
}

export interface SendInput extends PushRequestOptions {
  subscription: PushSubscriptionJson;
  /** Plaintext to encrypt. Omit for an empty push (no body, no Content-Encoding). */
  payload?: Uint8Array;
  vapid: { publicKey: string; keyPair: CryptoKeyPair; subject: string; nowSeconds?: number; expiresAt?: number };
  encrypt?: EncryptOptions;
}

export interface PushRequest {
  url: string;
  init: { method: "POST"; headers: Record<string, string>; body?: Bytes };
}

/** Builds the complete push request (encrypt + sign). Send it with `fetch(request.url, request.init)`. */
export async function buildPushRequest(input: SendInput): Promise<PushRequest> {
  const headers = pushHeaders(input);
  const audience = audienceOf(input.subscription.endpoint);
  const jwt = await createVapidJwt(
    { audience, subject: input.vapid.subject, ...(input.vapid.nowSeconds === undefined ? {} : { nowSeconds: input.vapid.nowSeconds }), ...(input.vapid.expiresAt === undefined ? {} : { expiresAt: input.vapid.expiresAt }) },
    input.vapid.keyPair.privateKey,
  );
  headers["Authorization"] = vapidAuthorization(jwt, input.vapid.publicKey);
  if (input.payload === undefined) return { url: input.subscription.endpoint, init: { method: "POST", headers } };
  const body = await encryptPayload(input.payload, b64uDecode(input.subscription.keys.p256dh), b64uDecode(input.subscription.keys.auth), input.encrypt);
  headers["Content-Encoding"] = "aes128gcm";
  headers["Content-Type"] = "application/octet-stream";
  return { url: input.subscription.endpoint, init: { method: "POST", headers, body } };
}

// ---------------------------------------------------------------------------------------------------------------
// Push service response handling

export type PushOutcome =
  | { action: "ok" }
  /** The subscription is gone for good (404, 410): delete it from storage. */
  | { action: "delete-subscription" }
  /** Transient: try again later, no sooner than `retryAfterSeconds` when the service gave one. */
  | { action: "retry"; retryAfterSeconds: number | null }
  /** Our request is wrong or the VAPID credentials are rejected: do not retry the same request, do not delete the subscription. */
  | { action: "drop"; reason: "bad-request" | "auth" | "payload-too-large" | "unexpected" };

/** Parses `Retry-After` (RFC 9110 10.2.3): delta seconds or an HTTP date. Returns whole seconds, or null if absent or invalid. */
export function parseRetryAfter(value: string | null | undefined, nowMs: number = Date.now()): number | null {
  if (value === null || value === undefined) return null;
  const text = value.trim();
  if (/^\d+$/.test(text)) return Number(text);
  const date = Date.parse(text);
  if (Number.isNaN(date)) return null;
  return Math.max(0, Math.ceil((date - nowMs) / 1000));
}

/** Maps a push service HTTP status to what the caller should do. Expected behaviour is documented in FINDINGS.md. */
export function classifyPushResponse(status: number, retryAfterHeader?: string | null, nowMs?: number): PushOutcome {
  if (status === 201 || status === 200 || status === 202) return { action: "ok" };
  if (status === 404 || status === 410) return { action: "delete-subscription" };
  if (status === 429 || (status >= 500 && status <= 599)) return { action: "retry", retryAfterSeconds: parseRetryAfter(retryAfterHeader, nowMs) };
  if (status === 400) return { action: "drop", reason: "bad-request" };
  if (status === 401 || status === 403) return { action: "drop", reason: "auth" };
  if (status === 413) return { action: "drop", reason: "payload-too-large" };
  return { action: "drop", reason: "unexpected" };
}

/** Seconds to wait before attempt number `attempt` (1 based): exponential from 30 s, capped at 1 h, never below Retry-After. */
export function retryDelaySeconds(attempt: number, retryAfterSeconds: number | null): number {
  const exponential = Math.min(30 * 2 ** Math.max(0, attempt - 1), 3600);
  return Math.max(exponential, retryAfterSeconds ?? 0);
}
