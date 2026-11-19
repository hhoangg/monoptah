// Rate limits of security.md 3.5 (A-07). This module has no routes: it is the `rateLimit(route)` middleware that
// `defineModule` runs after the browser checks and before the auth resolver, plus helpers for the limits that
// need data only a handler has (invite code, handle).
//
// Windows and limits live in `LIMITS`. Group limits are shared by every route of the group (`register/options`
// and `register/verify` draw from the same 10 per IP per hour, so one registration costs two calls).
// Exceeded: `429 {error, code: "rate_limited"}` with `Retry-After`; the breach is written to `audit_log` once
// per key per window, when the count first exceeds the limit, so a flood cannot turn into a D1 write flood.
import type { MiddlewareHandler } from "hono";
import type { ApiRoute, RateLimitGroup } from "@monoptah/protocol";
import { apiError, type AppContext, type AppEnv } from "../../http.ts";
import { sha256Hex } from "../../lib/crypto.ts";
import { clientIp, identifierHash, ipHash } from "../../lib/ip.ts";
import type { RATE_LIMIT_AUDIT_GROUPS } from "../audit/actions.ts";
import { auditStatement } from "../audit/audit.ts";
import { consume, rateLimitKey } from "./limiter.ts";
import { consumeIp } from "./memory.ts";

export { consume, rateLimitKey, type ConsumeResult } from "./limiter.ts";
export { consumeSession, resetIpLimits, resetSessionLimits, SESSION_LIMIT, SESSION_WINDOW_SECONDS } from "./memory.ts";
export { limitOutstandingPairings, MAX_OUTSTANDING_PAIRINGS } from "./pairings.ts";

type Group = (typeof RATE_LIMIT_AUDIT_GROUPS)[number];

export interface LimitSpec {
  limit: number;
  windowSeconds: number;
}

/** The table of security.md 3.5 (D1 rows; the general 120 per session per minute is in memory.ts). */
export const LIMITS = {
  /** `register/options` and `register/verify`: per IP. */
  register: { limit: 10, windowSeconds: 3600 },
  /** `login/options` and `login/verify`: per IP. */
  login: { limit: 30, windowSeconds: 600 },
  /** `auth/recover`: per IP. The per-handle limit is `handle`. */
  recover: { limit: 20, windowSeconds: 3600 },
  /** `pair/start`: per IP. "3 outstanding per IP" is `limitOutstandingPairings` (pairings.ts), called by the pair/start handler. */
  "pair-start": { limit: 5, windowSeconds: 600 },
  /** `pair/poll`: one call per `interval` (3 s) per device code. */
  "pair-poll": { limit: 1, windowSeconds: 3 },
  /** `push/notify`: per desktop token. */
  "push-notify": { limit: 30, windowSeconds: 60 },
  /**
   * Attempts per invite code across register/options and register/verify. The table gives no window; an
   * hour matches the other registration limit, and the invite code is hashed in the key.
   */
  invite: { limit: 5, windowSeconds: 3600 },
  /** Recovery attempts per handle ("5 per handle per hour"); the handle is hashed in the key. */
  handle: { limit: 5, windowSeconds: 3600 },
} as const satisfies Record<string, LimitSpec>;

/**
 * The per-IP limit applied to every group routed by IP, checked first. It is a total record over `RateLimitGroup`
 * (`null` = no per-IP limit), so a new protocol group without an entry fails typecheck. For `pair-poll` and `push-notify` the
 * per-code and per-token keys are chosen by the caller (a device code, a bearer token), so an attacker could
 * mint endless keys; this bound is per source address and cannot be dodged that way (WS-A addition to the table).
 */
export const IP_LIMITS = {
  register: LIMITS.register,
  login: LIMITS.login,
  recover: LIMITS.recover,
  "pair-start": LIMITS["pair-start"],
  "pair-poll": { limit: 120, windowSeconds: 60 },
  "push-notify": { limit: 120, windowSeconds: 60 },
} as const satisfies Record<RateLimitGroup, LimitSpec | null>;

/** `Retry-After` 429 for a refused call. */
function tooMany(c: AppContext, retryAfter: number): Response {
  c.header("Retry-After", String(retryAfter));
  return apiError(c, 429, "rate_limited", "Too many requests");
}

/**
 * Applies one D1 limit: counts the call under `key`, audits the first breach of the window and returns the
 * 429 response, or null when the call is within the limit.
 */
export async function enforce(c: AppContext, group: Group, key: string, spec: LimitSpec = LIMITS[group as keyof typeof LIMITS]): Promise<Response | null> {
  const result = await consume(c.env, key, spec.limit, spec.windowSeconds);
  if (result.ok) return null;
  if (result.firstBreach) await auditBreach(c, group, spec);
  return tooMany(c, result.retryAfter);
}

/** Writes the `ratelimit.breach` event. Never throws: a failed audit must not turn a 429 into a 500. */
export async function auditBreach(c: AppContext, group: Group, spec: LimitSpec, actorUserId?: string): Promise<void> {
  try {
    const hash = await ipHash(c.env, clientIp(c.req.raw));
    await auditStatement(c.env, {
      action: "ratelimit.breach",
      actorKind: actorUserId ? "user" : "system",
      actorUserId: actorUserId ?? null,
      meta: { group, limit: spec.limit, windowSeconds: spec.windowSeconds, ipHash: hash },
    }).run();
  } catch (err) {
    console.error(`rate limit audit failed: group=${group} error=${err instanceof Error ? err.name : "unknown"}`);
  }
}

/** Reads `deviceCode` from a JSON body without consuming the request body (the handler reads it again). */
async function deviceCodeOf(c: AppContext): Promise<string | null> {
  try {
    const body: unknown = await c.req.raw.clone().json();
    if (typeof body === "object" && body !== null && "deviceCode" in body && typeof body.deviceCode === "string" && body.deviceCode !== "") {
      return body.deviceCode;
    }
  } catch {
    // Not JSON: the handler answers 400.
  }
  return null;
}

/** The bearer token of the request, or null. */
function bearerOf(c: AppContext): string | null {
  const match = /^Bearer\s+(\S+)$/i.exec(c.req.header("Authorization") ?? "");
  return match?.[1] ?? null;
}

/**
 * Rate limit hook, one per route. Routes without `route.rateLimit` pass through (the general session limit runs
 * in the auth resolver, after the session is known). Keys hold hashes only:
 * - Every group: `group:ip:<HMAC of the IP>` with `IP_LIMITS`, behind an in-memory pre-filter (memory.ts).
 * - `pair-poll`, after the IP bound: `pair-poll:device:<SHA-256 of the deviceCode>`. A body without a
 *   `deviceCode` is left to the handler (400); no per-code key is consumed.
 * - `push-notify`, after the IP bound: `push-notify:token:<SHA-256 of the bearer token>`; without a bearer
 *   token only the IP bound applies.
 */
export function rateLimit(route: ApiRoute): MiddlewareHandler<AppEnv> {
  const group = route.rateLimit;
  if (!group) {
    return async (_c, next) => {
      await next();
    };
  }
  return async (c, next) => {
    const spec: LimitSpec | null = IP_LIMITS[group];
    if (!spec) return next();
    const hash = await ipHash(c.env, clientIp(c.req.raw));
    const ipKey = rateLimitKey(group, "ip", hash);
    // Best-effort pre-filter with the same limit as the D1 per-IP key. A call it refuses never reaches D1; the
    // call that first exceeds the limit does, so the breach is still audited once.
    const local = consumeIp(ipKey, spec.limit, spec.windowSeconds);
    if (!local.ok && !local.firstBreach) return tooMany(c, local.retryAfter);
    const limited = await enforce(c, group, ipKey, spec);
    if (limited) return limited;

    let key: string | null = null;
    if (group === "pair-poll") {
      const deviceCode = await deviceCodeOf(c);
      key = deviceCode === null ? null : rateLimitKey(group, "device", await sha256Hex(deviceCode));
    } else if (group === "push-notify") {
      const token = bearerOf(c);
      key = token === null ? null : rateLimitKey(group, "token", await sha256Hex(token));
    }
    if (key !== null) {
      const refused = await enforce(c, group, key);
      if (refused) return refused;
    }
    return next();
  };
}

/**
 * Per invite code: 5 attempts per hour (security.md 3.5). A-03 (register options/verify) and A-06 call it with the
 * code from the request, before looking the code up. Returns the 429 response to return, or null.
 */
export async function limitInviteAttempts(c: AppContext, inviteCode: string): Promise<Response | null> {
  return limitInviteAttemptsByHash(c, await sha256Hex(inviteCode));
}

/**
 * Per invite code by its SHA-256 (`invites.code_hash`): `register/verify` has only that hash, from the challenge
 * payload, and counts against the same key as `register/options`, so both routes draw from one 5 per hour budget.
 */
export async function limitInviteAttemptsByHash(c: AppContext, inviteCodeHash: string): Promise<Response | null> {
  return enforce(c, "invite", rateLimitKey("invite", "code", await identifierHash(c.env, "invite", inviteCodeHash)));
}

/**
 * Per handle: 5 recovery attempts per hour (security.md 3.5). A-05 calls it with the handle of the request,
 * known or not, so the answer does not reveal whether the handle exists. Returns the 429 response or null.
 */
export async function limitHandleAttempts(c: AppContext, handle: string): Promise<Response | null> {
  return enforce(c, "handle", rateLimitKey("handle", "handle", await identifierHash(c.env, "handle", handle.toLowerCase())));
}
