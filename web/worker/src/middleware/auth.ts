// Auth resolver, one per route (architecture.md 4.1, security.md 3.3). Resolves the cookie session for the
// routes of `auth` kind `session`, `fresh`, `admin` and `session+grant`, puts the principal on the Hono context
// (`c.get("auth")`, read it with `requireAuth(c)`), and answers 401 or 403. Then it applies the general
// 120-per-session-per-minute limit, or the separate 300-per-minute avatar bucket for `GET /api/desktops/:id/avatar`
// (in memory, best effort, see modules/ratelimit/memory.ts).
//
// - `public`: nothing is resolved and the cookie is never read.
// - `session`: cookie present, row exists, not revoked, before `idle_expires_at` and `expires_at`, user exists
//   and is `active`. Missing or invalid: 401 `unauthorized` and the cookie is cleared. Blocked: 403 `blocked`.
//   A valid session is touched (`last_seen_at`, idle expiry) at most once per hour.
// - `recovery_only` sessions get 403 `recovery_only` everywhere except `RECOVERY_ONLY_ALLOWLIST` and routes with
//   `allowRecoveryOnly` in the protocol registry (add a passkey). On those routes a recovery session counts as fresh.
// - `fresh`: a session with `reauth_at` within the last 5 minutes, else 403 `fresh_required`.
// - `admin`: `role = 'admin'` (else 403 `forbidden`) and fresh (else 403 `fresh_required`).
// - `session+grant` (relay device upgrade): only the session part is resolved here; WS-R checks the grant.
// - `token:*` (desktop bearer routes, R-03): a documented pass-through. WS-R resolves the bearer token. Cookies
//   never reach these routes: `browser.ts` strips the `Cookie` header before this middleware runs.
// - `freshWhen` (grants PUT, WS-R): the route is `session`; the handler calls `requireFresh(c)` when needed.
import type { MiddlewareHandler } from "hono";
import type { ApiRoute } from "@monoptah/protocol";
import { apiError, type AppContext, type AppEnv } from "../http.ts";
import { sha256Hex } from "../lib/crypto.ts";
import { nowSeconds } from "../lib/time.ts";
import { AVATAR_ROUTE_KEY, AVATAR_SESSION_LIMIT, auditBreach, consumeAvatarSession, consumeSession } from "../modules/ratelimit/index.ts";
import {
  SESSION_LIMIT,
  SESSION_WINDOW_SECONDS,
  type MemoryLimitResult,
} from "../modules/ratelimit/memory.ts";
import { clearSessionCookie, FRESH_SECONDS, IDLE_SECONDS, readSessionToken, TOUCH_INTERVAL_SECONDS, type SessionAmr } from "../modules/sessions/session.ts";

export interface AuthUser {
  id: string;
  handle: string;
  displayName: string;
  role: "user" | "admin";
}

export interface AuthSession {
  /** `SHA-256(cookie token)`: the row key and the `sessionIdHash` of `room.revoke`. */
  idHash: string;
  userId: string;
  amr: SessionAmr;
  recoveryOnly: boolean;
  createdAt: number;
  lastSeenAt: number;
  idleExpiresAt: number;
  expiresAt: number;
  reauthAt: number | null;
}

/** What the resolver puts on the context for `session`, `fresh`, `admin` and `session+grant` routes. */
export interface Principal {
  user: AuthUser;
  session: AuthSession;
}

/**
 * Routes a `recovery_only` session may call besides the ones marked `allowRecoveryOnly` in the protocol registry:
 * the recovery status (`GET /api/me`) and sign-out. Everything else answers 403 `recovery_only`.
 */
export const RECOVERY_ONLY_ALLOWLIST: ReadonlySet<string> = new Set(["GET /api/me", "POST /api/auth/logout"]);

export const SESSION_LOOKUP_SQL =
  "SELECT s.id_hash, s.user_id, s.created_at, s.last_seen_at, s.idle_expires_at, s.expires_at, s.reauth_at, s.amr, s.recovery_only, s.revoked_at, u.handle, u.display_name, u.role, u.status FROM sessions s JOIN users u ON u.id = s.user_id WHERE s.id_hash = ?";
/** Conditional on the old `last_seen_at`, so two concurrent requests write once. */
export const SESSION_TOUCH_SQL = "UPDATE sessions SET last_seen_at = ?, idle_expires_at = min(?, expires_at) WHERE id_hash = ? AND last_seen_at <= ?";

interface SessionRow {
  id_hash: string;
  user_id: string;
  created_at: number;
  last_seen_at: number;
  idle_expires_at: number;
  expires_at: number;
  reauth_at: number | null;
  amr: SessionAmr;
  recovery_only: number;
  revoked_at: number | null;
  handle: string;
  display_name: string;
  role: "user" | "admin";
  status: "active" | "blocked";
}

/** The principal of the request. Throws when called on a route whose resolver did not set one (a bug). */
export function requireAuth(c: AppContext): Principal {
  const auth = c.get("auth");
  if (!auth) throw new Error("No principal: the route is not session authenticated");
  return auth;
}

/** True when the session passed a passkey re-auth within the last 5 minutes. */
export function isFresh(session: Pick<AuthSession, "reauthAt">, now: number = nowSeconds()): boolean {
  return session.reauthAt !== null && session.reauthAt >= now - FRESH_SECONDS;
}

/**
 * For handlers that need a fresh session only for some requests (`freshWhen`, for example revoking a grant):
 * returns the 403 `fresh_required` response to return, or null when the session is fresh.
 */
export function requireFresh(c: AppContext): Response | null {
  return isFresh(requireAuth(c).session) ? null : apiError(c, 403, "fresh_required", "Re-authentication required");
}

function unauthorized(c: AppContext, hadCookie: boolean): Response {
  if (hadCookie) c.header("Set-Cookie", clearSessionCookie(), { append: true });
  return apiError(c, 401, "unauthorized", "Sign in required");
}

/** Resolves the cookie session to a principal, or null when it is missing, unknown, revoked or expired. */
async function resolveSession(c: AppContext, now: number): Promise<{ principal: Principal; status: "active" | "blocked" } | null> {
  const token = readSessionToken(c.req.raw);
  if (token === null) return null;
  const row = await c.env.DB.prepare(SESSION_LOOKUP_SQL).bind(await sha256Hex(token)).first<SessionRow>();
  if (!row || row.revoked_at !== null || now >= row.idle_expires_at || now >= row.expires_at) return null;
  return {
    status: row.status,
    principal: {
      user: { id: row.user_id, handle: row.handle, displayName: row.display_name, role: row.role },
      session: {
        idHash: row.id_hash,
        userId: row.user_id,
        amr: row.amr,
        recoveryOnly: row.recovery_only === 1,
        createdAt: row.created_at,
        lastSeenAt: row.last_seen_at,
        idleExpiresAt: row.idle_expires_at,
        expiresAt: row.expires_at,
        reauthAt: row.reauth_at,
      },
    },
  };
}

/** Writes `last_seen_at` and extends the idle expiry, once per hour per session. Mutates the principal. */
async function touch(c: AppContext, session: AuthSession, now: number): Promise<void> {
  if (now - session.lastSeenAt < TOUCH_INTERVAL_SECONDS) return;
  const idle = now + IDLE_SECONDS;
  const result = await c.env.DB.prepare(SESSION_TOUCH_SQL).bind(now, idle, session.idHash, now - TOUCH_INTERVAL_SECONDS).run();
  if ((result.meta.changes ?? 0) > 0) {
    session.lastSeenAt = now;
    session.idleExpiresAt = Math.min(idle, session.expiresAt);
  }
}

interface SessionBucket {
  group: "session" | "session-avatar";
  limit: number;
  consume: (idHash: string) => MemoryLimitResult;
}

const GENERAL_BUCKET: SessionBucket = { group: "session", limit: SESSION_LIMIT, consume: (idHash) => consumeSession(idHash) };
/** Avatar GETs do not count toward the general limit (security.md 3.5). */
const AVATAR_BUCKET: SessionBucket = { group: "session-avatar", limit: AVATAR_SESSION_LIMIT, consume: (idHash) => consumeAvatarSession(idHash) };

/** The general limit (120 per session per minute) or the avatar one (300), in memory. The first breach of a window is audited. */
async function applySessionLimit(c: AppContext, principal: Principal, bucket: SessionBucket): Promise<Response | null> {
  const result = bucket.consume(principal.session.idHash);
  if (result.ok) return null;
  if (result.firstBreach) {
    await auditBreach(c, bucket.group, { limit: bucket.limit, windowSeconds: SESSION_WINDOW_SECONDS }, principal.user.id);
  }
  c.header("Retry-After", String(result.retryAfter));
  return apiError(c, 429, "rate_limited", "Too many requests");
}

export function authResolver(route: ApiRoute): MiddlewareHandler<AppEnv> {
  const cookieAuth = route.auth === "session" || route.auth === "fresh" || route.auth === "admin" || route.auth === "session+grant";
  if (!cookieAuth) {
    // `public`: no cookie is read. `token:*`: WS-R (R-03) resolves the bearer token; the cookie was stripped.
    return (_c, next) => next();
  }
  const routeKey = `${route.method} ${route.path}`;
  const bucket = routeKey === AVATAR_ROUTE_KEY ? AVATAR_BUCKET : GENERAL_BUCKET;
  const recoveryOnlyAllowed = route.allowRecoveryOnly === true || RECOVERY_ONLY_ALLOWLIST.has(routeKey);

  return async (c, next) => {
    const now = nowSeconds();
    const hadCookie = readSessionToken(c.req.raw) !== null;
    const resolved = await resolveSession(c, now);
    if (!resolved) return unauthorized(c, hadCookie);
    const { principal, status } = resolved;
    if (status === "blocked") return apiError(c, 403, "blocked", "Account blocked");

    const limited = await applySessionLimit(c, principal, bucket);
    if (limited) return limited;

    if (principal.session.recoveryOnly) {
      if (!recoveryOnlyAllowed) return apiError(c, 403, "recovery_only", "Add a passkey first");
    }
    if (route.auth === "admin" && principal.user.role !== "admin") return apiError(c, 403, "forbidden", "Not allowed");
    const needsFresh = route.auth === "fresh" || route.auth === "admin";
    // A recovery session may call the add-passkey routes without a re-auth: it has no passkey to assert with.
    const recoveryException = principal.session.recoveryOnly && route.allowRecoveryOnly === true;
    if (needsFresh && !recoveryException && !isFresh(principal.session, now)) {
      return apiError(c, 403, "fresh_required", "Re-authentication required");
    }

    await touch(c, principal.session, now);
    c.set("auth", principal);
    return next();
  };
}
