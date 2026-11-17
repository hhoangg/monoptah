// Desktop bearer tokens (architecture.md 4.2 "token" auth, 4.3 `desktop_tokens`, 6.4). The token is 256 random
// bits, returned once by `pair/poll` and stored only as its SHA-256. Every token route and the desktop relay
// upgrade resolve it here.
import { apiError, type AppContext } from "../../http.ts";
import { sha256Hex } from "../../lib/crypto.ts";
import { nowSeconds } from "../../lib/time.ts";
import { consumeIp, type MemoryLimitResult } from "../ratelimit/memory.ts";

export type TokenScope = "relay" | "push" | "grants";

/** Every scope a pairing mints (architecture.md 4.3 `desktop_tokens.scope`). */
export const TOKEN_SCOPES = "relay push grants";

/** `last_used_at` is written at most once per this many seconds per token, to spare the D1 write budget. */
export const TOKEN_TOUCH_INTERVAL_SECONDS = 86_400;

/** In-memory per-token limits (no D1 write): API routes and relay upgrades, per minute, per isolate. */
export const TOKEN_API_LIMIT = 60;
export const TOKEN_RELAY_LIMIT = 12;
export const TOKEN_LIMIT_WINDOW_SECONDS = 60;

export interface DesktopPrincipal {
  tokenId: string;
  desktopId: string;
  userId: string;
}

/** Why a token was refused. Maps to HTTP (401, 403) on API routes and to a close code on the relay upgrade. */
export type TokenRefusal = "missing" | "invalid" | "scope" | "revoked" | "blocked";

export type TokenResolution = { ok: true; principal: DesktopPrincipal } | { ok: false; reason: TokenRefusal };

export const TOKEN_LOOKUP_SQL =
  "SELECT t.id, t.desktop_id, t.scope, t.revoked_at, t.last_used_at, d.user_id, d.revoked_at AS desktop_revoked_at, u.status FROM desktop_tokens t JOIN desktops d ON d.id = t.desktop_id JOIN users u ON u.id = d.user_id WHERE t.token_hash = ?";
/** Conditional on the old `last_used_at`, so concurrent requests write once per interval. */
export const TOKEN_TOUCH_SQL = "UPDATE desktop_tokens SET last_used_at = ? WHERE id = ? AND (last_used_at IS NULL OR last_used_at <= ?)";

interface TokenRow {
  id: string;
  desktop_id: string;
  scope: string;
  revoked_at: number | null;
  last_used_at: number | null;
  user_id: string;
  desktop_revoked_at: number | null;
  status: "active" | "blocked";
}

/** The bearer token of a request (`Authorization: Bearer <token>`), or null. */
export function bearerToken(request: Request): string | null {
  const match = /^Bearer\s+(\S+)$/i.exec(request.headers.get("Authorization") ?? "");
  return match?.[1] ?? null;
}

/**
 * Resolves the bearer token of `request` for `scope`: known hash, token not revoked, scope granted, desktop not
 * revoked, owner `active`. Touches `last_used_at` at most once per day.
 */
export async function resolveDesktopToken(
  env: Pick<Cloudflare.Env, "DB">,
  request: Request,
  scope: TokenScope,
  now: number = nowSeconds(),
  tokenHash?: string,
): Promise<TokenResolution> {
  const token = bearerToken(request);
  if (token === null) return { ok: false, reason: "missing" };
  const row = await env.DB.prepare(TOKEN_LOOKUP_SQL).bind(tokenHash ?? (await sha256Hex(token))).first<TokenRow>();
  if (!row) return { ok: false, reason: "invalid" };
  if (row.revoked_at !== null || row.desktop_revoked_at !== null) return { ok: false, reason: "revoked" };
  if (row.status !== "active") return { ok: false, reason: "blocked" };
  if (!row.scope.split(" ").includes(scope)) return { ok: false, reason: "scope" };
  if (row.last_used_at === null || now - row.last_used_at >= TOKEN_TOUCH_INTERVAL_SECONDS) {
    await env.DB.prepare(TOKEN_TOUCH_SQL).bind(now, row.id, now - TOKEN_TOUCH_INTERVAL_SECONDS).run();
  }
  return { ok: true, principal: { tokenId: row.id, desktopId: row.desktop_id, userId: row.user_id } };
}

/**
 * Counts one use of the bearer token of `request` (keyed by its SHA-256) against the in-memory limit of `kind`.
 * Null when there is no bearer token (the lookup answers 401). Writes nothing to D1.
 */
export async function consumeTokenLimit(request: Request, kind: "api" | "relay", now: number = nowSeconds()): Promise<{ result: MemoryLimitResult; hash: string } | null> {
  const token = bearerToken(request);
  if (token === null) return null;
  const hash = await sha256Hex(token);
  const limit = kind === "api" ? TOKEN_API_LIMIT : TOKEN_RELAY_LIMIT;
  return { result: consumeIp(`desktop-token:${kind}:${hash}`, limit, TOKEN_LIMIT_WINDOW_SECONDS, now), hash };
}

/** The API answer for a refused token: 401 `unauthorized` for a missing or unknown token, 403 otherwise. */
export function tokenRefusalResponse(c: AppContext, reason: TokenRefusal): Response {
  switch (reason) {
    case "missing":
    case "invalid":
      return apiError(c, 401, "unauthorized", "Desktop token required");
    case "blocked":
      return apiError(c, 403, "blocked", "Account blocked");
    case "revoked":
      return apiError(c, 403, "forbidden", "Desktop token revoked");
    case "scope":
      return apiError(c, 403, "forbidden", "Token scope not allowed");
  }
}

/**
 * For handlers of `token:<scope>` routes: the desktop principal, or the error response to return.
 * Usage: `const auth = await requireDesktopToken(c, "grants"); if (auth instanceof Response) return auth;`
 */
export async function requireDesktopToken(c: AppContext, scope: TokenScope): Promise<DesktopPrincipal | Response> {
  const limited = await consumeTokenLimit(c.req.raw, "api");
  if (limited && !limited.result.ok) {
    c.header("Retry-After", String(limited.result.retryAfter));
    return apiError(c, 429, "rate_limited", "Too many requests");
  }
  const resolved = await resolveDesktopToken(c.env, c.req.raw, scope, nowSeconds(), limited?.hash);
  return resolved.ok ? resolved.principal : tokenRefusalResponse(c, resolved.reason);
}
