import type { Env } from "../env.ts";
import { hmacSha256Hex } from "./crypto.ts";

/** Placeholder when the request carries no client address (local tests, `wrangler dev` without a proxy). */
export const UNKNOWN_IP = "unknown";

/** The client address as seen by Cloudflare (`CF-Connecting-IP`), or `"unknown"`. Never stored raw. */
export function clientIp(request: Request): string {
  const value = request.headers.get("CF-Connecting-IP")?.trim();
  return value ? value : UNKNOWN_IP;
}

/**
 * Salted hash of an IP address: HMAC-SHA-256 with `AUDIT_IP_SALT`, as lowercase hex (security.md 10).
 * This is the only form in which an IP is written to D1 (audit `meta.ipHash`, `pairing_requests.ip_hash`)
 * or put in a rate limit key.
 */
export function ipHash(env: Pick<Env, "AUDIT_IP_SALT">, ip: string): Promise<string> {
  return hmacSha256Hex(env.AUDIT_IP_SALT, ip);
}

/**
 * Salted hash of a low-entropy identifier (a handle, an invite code) for use in a rate limit key, so the
 * identifier never appears in D1. Namespaced so the same string under two namespaces gives different hashes.
 */
export function identifierHash(env: Pick<Env, "AUDIT_IP_SALT">, namespace: string, value: string): Promise<string> {
  return hmacSha256Hex(env.AUDIT_IP_SALT, `${namespace}:${value}`);
}
