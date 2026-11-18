// VAPID (RFC 8292): ES256 JWT and the `Authorization: vapid` header. Keys come from the env: the current pair
// (`VAPID_*`) and an optional previous pair for a dual-key send period (P-06, security.md "VAPID rotation runbook").
import { toBase64Url } from "../../lib/crypto.ts";
import { nowSeconds } from "../../lib/time.ts";

/** The key pairs the push code reads. The previous pair is optional (declared in `cloudflare-env.d.ts`, set only during a rotation). */
export interface VapidEnv {
  VAPID_PUBLIC_KEY?: string;
  VAPID_PRIVATE_KEY?: string;
  VAPID_KEY_ID?: string;
  VAPID_SUBJECT?: string;
  VAPID_PREVIOUS_PUBLIC_KEY?: string;
  VAPID_PREVIOUS_PRIVATE_KEY?: string;
  VAPID_PREVIOUS_KEY_ID?: string;
}

export interface VapidKeyConfig {
  keyId: string;
  /** Base64url, 65-byte uncompressed P-256 point. */
  publicKey: string;
  /** Base64url, 32-byte scalar. */
  privateKey: string;
}

/** The JWT lives 12 h (RFC 8292 allows at most 24 h) and is reused until 1 h before it expires. */
export const JWT_LIFETIME_SECONDS = 12 * 3600;
export const JWT_REUSE_MARGIN_SECONDS = 3600;
const CACHE_LIMIT = 200;

export function fromBase64Url(value: string): Uint8Array<ArrayBuffer> {
  const padded = value.replaceAll("-", "+").replaceAll("_", "/").padEnd(Math.ceil(value.length / 4) * 4, "=");
  if (!/^[A-Za-z0-9+/]*={0,2}$/.test(padded)) throw new Error("Invalid base64url");
  const binary = atob(padded);
  const bytes = new Uint8Array(binary.length);
  for (let i = 0; i < binary.length; i++) bytes[i] = binary.charCodeAt(i);
  return bytes;
}

/** The current key, or null when a part is missing. */
export function currentKey(env: VapidEnv): VapidKeyConfig | null {
  if (!env.VAPID_PUBLIC_KEY || !env.VAPID_PRIVATE_KEY || !env.VAPID_KEY_ID) return null;
  return { keyId: env.VAPID_KEY_ID, publicKey: env.VAPID_PUBLIC_KEY, privateKey: env.VAPID_PRIVATE_KEY };
}

/** The previous key, or null when no rotation period is running. */
export function previousKey(env: VapidEnv): VapidKeyConfig | null {
  if (!env.VAPID_PREVIOUS_PUBLIC_KEY || !env.VAPID_PREVIOUS_PRIVATE_KEY || !env.VAPID_PREVIOUS_KEY_ID) return null;
  return { keyId: env.VAPID_PREVIOUS_KEY_ID, publicKey: env.VAPID_PREVIOUS_PUBLIC_KEY, privateKey: env.VAPID_PREVIOUS_PRIVATE_KEY };
}

/** Imports the private key through a JWK (`d` from the secret, `x` and `y` from the public point). Throws when invalid. */
export async function importSigningKey(key: VapidKeyConfig): Promise<CryptoKey> {
  const point = fromBase64Url(key.publicKey);
  const d = fromBase64Url(key.privateKey);
  if (point.length !== 65 || point[0] !== 4 || d.length !== 32) throw new Error("Invalid VAPID key");
  const jwk: JsonWebKey = {
    kty: "EC",
    crv: "P-256",
    x: toBase64Url(point.subarray(1, 33)),
    y: toBase64Url(point.subarray(33, 65)),
    d: key.privateKey,
  };
  return crypto.subtle.importKey("jwk", jwk, { name: "ECDSA", namedCurve: "P-256" }, false, ["sign"]);
}

interface CachedJwt {
  jwt: string;
  exp: number;
}

/** Signed JWTs per isolate by (audience, key id, public key). Bounded. */
const jwtCache = new Map<string, CachedJwt>();
const keyCache = new Map<string, Promise<CryptoKey>>();

function bounded<V>(map: Map<string, V>, key: string, value: V): void {
  if (map.size >= CACHE_LIMIT) map.delete(map.keys().next().value as string);
  map.set(key, value);
}

const encoder = new TextEncoder();

/** The imported signing key, cached per isolate by (key id, public key, private key). A failed import is not kept. */
function signingKeyOf(key: VapidKeyConfig): Promise<CryptoKey> {
  const id = `${key.keyId}|${key.publicKey}|${key.privateKey}`;
  let signing = keyCache.get(id);
  if (!signing) {
    signing = importSigningKey(key);
    signing.catch(() => keyCache.delete(id));
    bounded(keyCache, id, signing);
  }
  return signing;
}

/** ES256 JWT for `audience` (the origin of the push endpoint), cached until 1 h before its `exp`. */
export async function vapidJwt(key: VapidKeyConfig, subject: string, audience: string, now: number = nowSeconds()): Promise<string> {
  const cacheKey = `${audience}|${key.keyId}|${key.publicKey}|${subject}`;
  const hit = jwtCache.get(cacheKey);
  if (hit && hit.exp - JWT_REUSE_MARGIN_SECONDS > now) return hit.jwt;
  const signing = signingKeyOf(key);
  const exp = now + JWT_LIFETIME_SECONDS;
  const header = toBase64Url(encoder.encode(JSON.stringify({ typ: "JWT", alg: "ES256" })));
  const claims = toBase64Url(encoder.encode(JSON.stringify({ aud: audience, exp, sub: subject })));
  const signingInput = `${header}.${claims}`;
  // WebCrypto ECDSA returns the raw r||s form that JWS ES256 uses.
  const signature = new Uint8Array(await crypto.subtle.sign({ name: "ECDSA", hash: "SHA-256" }, await signing, encoder.encode(signingInput)));
  const jwt = `${signingInput}.${toBase64Url(signature)}`;
  bounded(jwtCache, cacheKey, { jwt, exp });
  return jwt;
}

/** The `Authorization` header value of a push request. */
export async function vapidAuthorization(key: VapidKeyConfig, subject: string, endpoint: string, now?: number): Promise<string> {
  const jwt = await vapidJwt(key, subject, new URL(endpoint).origin, now);
  return `vapid t=${jwt}, k=${key.publicKey}`;
}

/** True when the key imports (not the placeholder, 32-byte `d` matching a 65-byte point). */
export async function keyIsUsable(key: VapidKeyConfig | null): Promise<boolean> {
  if (!key) return false;
  try {
    await signingKeyOf(key);
    return true;
  } catch {
    return false;
  }
}

/** Test hook: empties the per-isolate caches. */
export function resetVapidCaches(): void {
  jwtCache.clear();
  keyCache.clear();
}
