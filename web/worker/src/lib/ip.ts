import type { Env } from "../env.ts";
import { hmacSha256Hex } from "./crypto.ts";

/** Placeholder when the request carries no client address (local tests, `wrangler dev` without a proxy). */
export const UNKNOWN_IP = "unknown";

/** The client address as seen by Cloudflare (`CF-Connecting-IP`), or `"unknown"`. Never stored raw. */
export function clientIp(request: Request): string {
  const value = request.headers.get("CF-Connecting-IP")?.trim();
  return value ? value : UNKNOWN_IP;
}

function parseIpv4(text: string): number[] | null {
  const parts = text.split(".");
  if (parts.length !== 4) return null;
  const octets = parts.map((part) => (/^\d{1,3}$/.test(part) ? Number(part) : NaN));
  return octets.every((o) => o <= 255) ? octets : null;
}

/** The eight 16-bit groups of an IPv6 text address (`::` expanded, an embedded IPv4 tail converted), or null. */
function parseIpv6(text: string): number[] | null {
  let body = text;
  const zone = body.indexOf("%");
  if (zone >= 0) body = body.slice(0, zone);
  const tailStart = body.lastIndexOf(":") + 1;
  if (body.slice(tailStart).includes(".")) {
    const v4 = parseIpv4(body.slice(tailStart));
    if (!v4) return null;
    const hex = (a: number, b: number) => ((a << 8) | b).toString(16);
    body = `${body.slice(0, tailStart)}${hex(v4[0] ?? 0, v4[1] ?? 0)}:${hex(v4[2] ?? 0, v4[3] ?? 0)}`;
  }
  const halves = body.split("::");
  if (halves.length > 2) return null;
  const toGroups = (part: string): string[] => (part === "" ? [] : part.split(":"));
  const head = toGroups(halves[0] ?? "");
  const tail = halves.length === 2 ? toGroups(halves[1] ?? "") : [];
  const missing = 8 - head.length - tail.length;
  if (halves.length === 2 ? missing < 1 : missing !== 0) return null;
  const groups = [...head, ...Array<string>(halves.length === 2 ? missing : 0).fill("0"), ...tail];
  if (groups.length !== 8 || !groups.every((g) => /^[0-9a-f]{1,4}$/.test(g))) return null;
  return groups.map((g) => parseInt(g, 16));
}

/**
 * The key an address is limited and recorded under: an IPv4 address as is, an IPv6 address as its /64 prefix
 * (`2001:db8:1:2::/64`), because one subscriber normally holds a whole /64 and could otherwise mint endless keys.
 * The text is normalized first (lowercase, `::` expanded, leading zeros dropped, zone id dropped), and an
 * IPv4-mapped address (`::ffff:a.b.c.d`) is the IPv4 address it carries. Anything that does not parse is kept
 * as lowercase text (`"unknown"` included), so it still gets a key.
 */
export function ipKey(ip: string): string {
  const text = ip.trim().toLowerCase();
  if (!text.includes(":")) return text;
  const groups = parseIpv6(text);
  if (!groups) return text;
  if (groups.slice(0, 5).every((g) => g === 0) && groups[5] === 0xffff) {
    const [hi = 0, lo = 0] = groups.slice(6);
    return `${hi >> 8}.${hi & 255}.${lo >> 8}.${lo & 255}`;
  }
  return `${groups.slice(0, 4).map((g) => g.toString(16)).join(":")}::/64`;
}

/**
 * Salted hash of an IP address: HMAC-SHA-256 with `AUDIT_IP_SALT`, as lowercase hex (security.md 10), of its
 * `ipKey` (IPv6 by /64 prefix).
 * This is the only form in which an IP is written to D1 (audit `meta.ipHash`, `pairing_requests.ip_hash`)
 * or put in a rate limit key.
 */
export function ipHash(env: Pick<Env, "AUDIT_IP_SALT">, ip: string): Promise<string> {
  return hmacSha256Hex(env.AUDIT_IP_SALT, ipKey(ip));
}

/**
 * Salted hash of a low-entropy identifier (a handle, an invite code) for use in a rate limit key, so the
 * identifier never appears in D1. Namespaced so the same string under two namespaces gives different hashes.
 */
export function identifierHash(env: Pick<Env, "AUDIT_IP_SALT">, namespace: string, value: string): Promise<string> {
  return hmacSha256Hex(env.AUDIT_IP_SALT, `${namespace}:${value}`);
}
