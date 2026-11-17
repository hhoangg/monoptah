// Helpers for the push tests: key pairs generated at test time, a subscriber that can decrypt (RFC 8291 receiver
// side, written independently of the sender with plain HMAC based HKDF), rows, and a fetch double.
import { env } from "cloudflare:workers";
import { toBase64Url, sha256Hex } from "../../../src/lib/crypto.ts";
import type { PushFetch } from "../../../src/modules/push/webpush.ts";
import { createTestDesktop, createTestUser } from "../../support/accounts.ts";

type TestEnv = typeof env;
const encoder = new TextEncoder();

export function b64(value: string): Uint8Array<ArrayBuffer> {
  const padded = value.replaceAll("-", "+").replaceAll("_", "/").padEnd(Math.ceil(value.length / 4) * 4, "=");
  const binary = atob(padded);
  return Uint8Array.from(binary, (ch) => ch.charCodeAt(0));
}

export interface Pair {
  /** Base64url 65-byte point. */
  publicKey: string;
  /** Base64url 32-byte scalar. */
  privateKey: string;
  privateCrypto: CryptoKey;
  publicRaw: Uint8Array<ArrayBuffer>;
}

/** A fresh P-256 pair usable for ECDSA (VAPID) and, with `usage`, ECDH (a subscriber). */
export async function generatePair(algorithm: "ECDSA" | "ECDH" = "ECDSA"): Promise<Pair> {
  const usages: string[] = algorithm === "ECDSA" ? ["sign", "verify"] : ["deriveBits"];
  const pair = (await crypto.subtle.generateKey({ name: algorithm, namedCurve: "P-256" }, true, usages as never)) as CryptoKeyPair;
  const jwk = (await crypto.subtle.exportKey("jwk", pair.privateKey)) as JsonWebKey;
  const publicRaw = new Uint8Array((await crypto.subtle.exportKey("raw", pair.publicKey)) as ArrayBuffer);
  return { publicKey: toBase64Url(publicRaw), privateKey: jwk.d as string, privateCrypto: pair.privateKey, publicRaw };
}

/** Imports an ECDH private key from a raw public point and base64url `d` (the RFC 8291 Appendix A keys). */
export async function ecdhPrivateFromParts(publicRaw: Uint8Array, d: string): Promise<CryptoKey> {
  const jwk: JsonWebKey = { kty: "EC", crv: "P-256", x: toBase64Url(publicRaw.subarray(1, 33)), y: toBase64Url(publicRaw.subarray(33, 65)), d };
  return crypto.subtle.importKey("jwk", jwk, { name: "ECDH", namedCurve: "P-256" }, true, ["deriveBits"]);
}

async function hmac(key: Uint8Array<ArrayBuffer>, data: Uint8Array<ArrayBuffer>): Promise<Uint8Array<ArrayBuffer>> {
  const k = await crypto.subtle.importKey("raw", key, { name: "HMAC", hash: "SHA-256" }, false, ["sign"]);
  return new Uint8Array(await crypto.subtle.sign("HMAC", k, data));
}

function cat(...parts: Uint8Array[]): Uint8Array<ArrayBuffer> {
  const out = new Uint8Array(parts.reduce((n, p) => n + p.length, 0));
  let o = 0;
  for (const p of parts) {
    out.set(p, o);
    o += p.length;
  }
  return out;
}

export interface Subscriber {
  p256dh: string;
  auth: string;
  /** Decrypts a body the sender produced for this subscriber; returns the plaintext without the padding delimiter. */
  decrypt(body: Uint8Array): Promise<Uint8Array>;
}

/** The RFC 8291 section 3.4 pseudocode for the user agent. */
export async function decryptAs(uaPrivate: CryptoKey, uaPublic: Uint8Array<ArrayBuffer>, authSecret: Uint8Array<ArrayBuffer>, body: Uint8Array): Promise<Uint8Array> {
  const salt = body.slice(0, 16);
  const rs = new DataView(body.buffer, body.byteOffset + 16, 4).getUint32(0);
  const idLen = body[20] ?? 0;
  const asPublic = body.slice(21, 21 + idLen);
  const ciphertext = body.slice(21 + idLen);
  if (rs !== 4096 || idLen !== 65) throw new Error("Unexpected header");
  const asKey = await crypto.subtle.importKey("raw", asPublic, { name: "ECDH", namedCurve: "P-256" }, false, []);
  const ecdh = new Uint8Array(await crypto.subtle.deriveBits({ name: "ECDH", public: asKey } as unknown as SubtleCryptoDeriveKeyAlgorithm, uaPrivate, 256));
  const prkKey = await hmac(authSecret, ecdh);
  const ikm = await hmac(prkKey, cat(encoder.encode("WebPush: info\0"), uaPublic, asPublic, new Uint8Array([1])));
  const prk = await hmac(salt, ikm);
  const cek = (await hmac(prk, cat(encoder.encode("Content-Encoding: aes128gcm\0"), new Uint8Array([1])))).slice(0, 16);
  const nonce = (await hmac(prk, cat(encoder.encode("Content-Encoding: nonce\0"), new Uint8Array([1])))).slice(0, 12);
  const key = await crypto.subtle.importKey("raw", cek, "AES-GCM", false, ["decrypt"]);
  const padded = new Uint8Array(await crypto.subtle.decrypt({ name: "AES-GCM", iv: nonce }, key, ciphertext));
  if (padded[padded.length - 1] !== 2) throw new Error("Missing padding delimiter");
  return padded.slice(0, -1);
}

export async function newSubscriber(): Promise<Subscriber> {
  const pair = await generatePair("ECDH");
  const authSecret = crypto.getRandomValues(new Uint8Array(16));
  return {
    p256dh: pair.publicKey,
    auth: toBase64Url(authSecret),
    decrypt: (body) => decryptAs(pair.privateCrypto, pair.publicRaw, authSecret, body),
  };
}

/** The environment with values replaced. Own properties, so the real `env` is not changed for later tests. */
export function overlayEnv(values: Record<string, unknown>): TestEnv {
  const own = Object.fromEntries(Object.entries(values).map(([key, value]) => [key, { value, enumerable: true, writable: true, configurable: true }]));
  return Object.create(env, own) as TestEnv;
}

export interface VapidSetup {
  env: TestEnv;
  current: Pair;
  previous: Pair;
}

/** An env with a generated current key (id "cur") and a generated previous key (id "old" present only when asked). */
export async function vapidEnv(extra: { withPrevious?: boolean } = {}): Promise<VapidSetup> {
  const current = await generatePair();
  const previous = await generatePair();
  const values: Record<string, unknown> = {
    VAPID_PUBLIC_KEY: current.publicKey,
    VAPID_PRIVATE_KEY: current.privateKey,
    VAPID_KEY_ID: "cur",
    VAPID_SUBJECT: "mailto:test@example.invalid",
  };
  if (extra.withPrevious) {
    values["VAPID_PREVIOUS_PUBLIC_KEY"] = previous.publicKey;
    values["VAPID_PREVIOUS_PRIVATE_KEY"] = previous.privateKey;
    values["VAPID_PREVIOUS_KEY_ID"] = "old";
  }
  return { env: overlayEnv(values), current, previous };
}

export interface SentRequest {
  url: string;
  headers: Record<string, string>;
  body: Uint8Array;
}

/** A fetch double: records requests and answers with `status` (or a function of the request). */
export function fakeFetch(answer: number | ((url: string) => Response | Promise<Response>) = 201): { fetch: PushFetch; sent: SentRequest[] } {
  const sent: SentRequest[] = [];
  const fetch: PushFetch = async (url, init) => {
    sent.push({ url, headers: Object.fromEntries(new Headers(init.headers).entries()), body: new Uint8Array(init.body as ArrayBuffer) });
    return typeof answer === "number" ? new Response(null, { status: answer }) : answer(url);
  };
  return { fetch, sent };
}

// --- Rows ---

export async function createDevice(userId: string, options: { revoked?: boolean } = {}): Promise<string> {
  const id = crypto.randomUUID();
  await env.DB.prepare("INSERT INTO devices (id, user_id, name, public_key, fingerprint, created_at, revoked_at) VALUES (?, ?, 'Phone', 'key', 'fp', 1, ?)")
    .bind(id, userId, options.revoked ? 2 : null)
    .run();
  return id;
}

export async function setGrant(deviceId: string, desktopId: string, state: "pending" | "confirmed" | "revoked"): Promise<void> {
  await env.DB.prepare("INSERT OR REPLACE INTO device_grants (device_id, desktop_id, state, updated_at) VALUES (?, ?, ?, 1)").bind(deviceId, desktopId, state).run();
}

export async function setPrefs(userId: string, desktopId: string, enabled: boolean, categories: Record<string, boolean>): Promise<void> {
  await env.DB.prepare("INSERT OR REPLACE INTO push_prefs (user_id, desktop_id, enabled, categories) VALUES (?, ?, ?, ?)")
    .bind(userId, desktopId, enabled ? 1 : 0, JSON.stringify(categories))
    .run();
}

export async function createToken(desktopId: string, options: { scope?: string; revoked?: boolean } = {}): Promise<string> {
  const token = toBase64Url(crypto.getRandomValues(new Uint8Array(32)));
  await env.DB.prepare("INSERT INTO desktop_tokens (id, desktop_id, token_hash, scope, created_at, last_used_at, revoked_at) VALUES (?, ?, ?, ?, 1, ?, ?)")
    .bind(crypto.randomUUID(), desktopId, await sha256Hex(token), options.scope ?? "relay push grants", Math.floor(Date.now() / 1000), options.revoked ? 2 : null)
    .run();
  return token;
}

export interface SubscriptionRow {
  id: string;
  endpoint: string;
  vapid_key_id: string;
  failures: number;
  last_success_at: number | null;
  user_id: string;
  device_id: string;
}

export async function insertSubscription(
  userId: string,
  deviceId: string,
  subscriber: Subscriber,
  options: { endpoint?: string; vapidKeyId?: string; failures?: number; lastSuccessAt?: number | null } = {},
): Promise<string> {
  const id = crypto.randomUUID();
  await env.DB.prepare(
    "INSERT INTO push_subscriptions (id, user_id, device_id, endpoint, p256dh, auth, vapid_key_id, ua, created_at, last_success_at, failures) VALUES (?, ?, ?, ?, ?, ?, ?, NULL, 1, ?, ?)",
  )
    .bind(id, userId, deviceId, options.endpoint ?? `https://fcm.googleapis.com/fcm/send/${crypto.randomUUID()}`, subscriber.p256dh, subscriber.auth, options.vapidKeyId ?? "cur", options.lastSuccessAt ?? null, options.failures ?? 0)
    .run();
  return id;
}

export async function subscriptionRow(id: string): Promise<SubscriptionRow | null> {
  return env.DB.prepare("SELECT id, endpoint, vapid_key_id, failures, last_success_at, user_id, device_id FROM push_subscriptions WHERE id = ?").bind(id).first<SubscriptionRow>();
}

export interface World {
  userId: string;
  desktopId: string;
  deviceId: string;
  subscriber: Subscriber;
  subscriptionId: string;
  endpoint: string;
  token: string;
}

/** An owner with a desktop (named "Studio Mac"), push prefs on for all categories, one device with a confirmed grant and one subscription. */
export async function createWorld(options: { prefs?: Record<string, boolean> | null; vapidKeyId?: string } = {}): Promise<World> {
  const user = await createTestUser(env);
  const desktopId = await createTestDesktop(env, user.id);
  await env.DB.prepare("UPDATE desktops SET name = 'Studio Mac' WHERE id = ?").bind(desktopId).run();
  const deviceId = await createDevice(user.id);
  await setGrant(deviceId, desktopId, "confirmed");
  if (options.prefs !== null) await setPrefs(user.id, desktopId, true, options.prefs ?? { agentFinished: true, agentInput: true, monoMessages: true });
  const subscriber = await newSubscriber();
  const endpoint = `https://fcm.googleapis.com/fcm/send/${crypto.randomUUID()}`;
  const subscriptionId = await insertSubscription(user.id, deviceId, subscriber, { endpoint, vapidKeyId: options.vapidKeyId ?? "cur" });
  return { userId: user.id, desktopId, deviceId, subscriber, subscriptionId, endpoint, token: await createToken(desktopId) };
}
