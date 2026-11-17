// Cookie sessions (security.md 3.3, A-04): token and cookie construction, the INSERT of a session row for the
// caller's batch, and the revoke helpers used by logout, "sign out everywhere", recovery and the admin block.
//
// The cookie holds a random 256-bit token; D1 holds only `SHA-256(token)` as `sessions.id_hash`. Importing this
// file from a handler is fine; it imports nothing from `define.ts`, so there is no module cycle.
import { CloseCodes } from "@monoptah/protocol";
import type { Env } from "../../env.ts";
import { randomToken, sha256Hex } from "../../lib/crypto.ts";
import { nowSeconds } from "../../lib/time.ts";

export const SESSION_COOKIE = "__Host-mc_session";
/** Idle timeout, extended on use at most once per hour. */
export const IDLE_SECONDS = 7 * 86400;
/** Absolute lifetime. */
export const ABSOLUTE_SECONDS = 30 * 86400;
/** Absolute lifetime of a `recovery_only` session. */
export const RECOVERY_ONLY_SECONDS = 30 * 60;
/** A re-auth counts as "fresh" for 5 minutes. */
export const FRESH_SECONDS = 300;
/** `last_seen_at` and `idle_expires_at` are written at most once per this many seconds per session. */
export const TOUCH_INTERVAL_SECONDS = 3600;
export const USER_AGENT_MAX_LENGTH = 200;
/** Upper bound of `room.revoke` calls per revoke, to stay inside the subrequest limit of a request. */
export const MAX_REVOKE_CALLS = 40;

export type SessionAmr = "passkey" | "recovery";

export interface CreateSessionInput {
  userId: string;
  amr: SessionAmr;
  /** A recovery session: it can only add a passkey, and lasts 30 minutes. */
  recoveryOnly?: boolean;
  /** The `User-Agent` header, stored truncated to 200 characters. */
  userAgent?: string | null;
  /** Unix seconds of the passkey assertion that created the session (login sets it to now), or null. */
  reauthAt?: number | null;
  /** Unix seconds; defaults to now. */
  now?: number;
  /** Write the row only if this SQL condition holds (registration: only when the user row was created). */
  when?: { sql: string; params: unknown[] };
}

export interface CreatedSession {
  /** The cookie value. Shown only here: it is never stored. */
  token: string;
  /** `SHA-256(token)`, the row key, and the `sessionIdHash` of `room.revoke`. */
  idHash: string;
  /** `Max-Age` of the cookie, in seconds: the time to the absolute expiry. */
  maxAge: number;
  expiresAt: number;
  /** The INSERT, to include in the caller's `env.DB.batch([...])` (registration, login and recovery do). */
  statement: D1PreparedStatement;
  /** The `Set-Cookie` value for the response. */
  cookie: string;
}

export const INSERT_SESSION_SQL =
  "INSERT INTO sessions (id_hash, user_id, created_at, last_seen_at, idle_expires_at, expires_at, reauth_at, amr, recovery_only, user_agent) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)";

const INSERT_SESSION_WHEN_SQL = (condition: string): string =>
  `INSERT INTO sessions (id_hash, user_id, created_at, last_seen_at, idle_expires_at, expires_at, reauth_at, amr, recovery_only, user_agent) SELECT ?, ?, ?, ?, ?, ?, ?, ?, ?, ? WHERE ${condition}`;

/**
 * Builds a new session: a random token, its hash, and the INSERT statement (not run). Idle expiry is 7 days and
 * absolute expiry 30 days; a `recoveryOnly` session expires after 30 minutes in both.
 */
export async function createSession(env: Pick<Env, "DB">, input: CreateSessionInput): Promise<CreatedSession> {
  const now = input.now ?? nowSeconds();
  const recoveryOnly = input.recoveryOnly ?? false;
  const token = randomToken(32);
  const idHash = await sha256Hex(token);
  const expiresAt = now + (recoveryOnly ? RECOVERY_ONLY_SECONDS : ABSOLUTE_SECONDS);
  const idleExpiresAt = Math.min(now + IDLE_SECONDS, expiresAt);
  const maxAge = expiresAt - now;
  const values = [
    idHash,
    input.userId,
    now,
    now,
    idleExpiresAt,
    expiresAt,
    input.reauthAt ?? null,
    input.amr,
    recoveryOnly ? 1 : 0,
    input.userAgent ? input.userAgent.slice(0, USER_AGENT_MAX_LENGTH) : null,
  ];
  const statement = input.when
    ? env.DB.prepare(INSERT_SESSION_WHEN_SQL(input.when.sql)).bind(...values, ...input.when.params)
    : env.DB.prepare(INSERT_SESSION_SQL).bind(...values);
  return { token, idHash, maxAge, expiresAt, statement, cookie: sessionCookie(token, maxAge) };
}

/**
 * `Set-Cookie` value of the session cookie: `__Host-mc_session=<token>; HttpOnly; Secure; Path=/;
 * SameSite=Strict; Max-Age=<seconds>`. Never a `Domain` attribute (the `__Host-` prefix forbids it).
 */
export function sessionCookie(token: string, maxAge: number): string {
  return `${SESSION_COOKIE}=${token}; HttpOnly; Secure; Path=/; SameSite=Strict; Max-Age=${Math.max(0, Math.floor(maxAge))}`;
}

/** `Set-Cookie` value that removes the session cookie in the browser. */
export function clearSessionCookie(): string {
  return sessionCookie("", 0);
}

/** The session token of a request (`Cookie` header), or null. Reads only the `__Host-mc_session` cookie. */
export function readSessionToken(request: Request): string | null {
  const header = request.headers.get("Cookie");
  if (!header) return null;
  for (const part of header.split(";")) {
    const eq = part.indexOf("=");
    if (eq < 0) continue;
    if (part.slice(0, eq).trim() === SESSION_COOKIE) {
      const value = part.slice(eq + 1).trim();
      return value === "" ? null : value;
    }
  }
  return null;
}

/**
 * The id a client sees for a session in `GET /api/sessions` and `DELETE /api/sessions/:id`: the first 32 hex
 * characters of `SHA-256(id_hash)`. `id_hash` itself is one-way and cannot forge a cookie, but it is the key
 * the relay uses to close sockets (`room.revoke`), so the API exposes a derived id instead and keeps the
 * stored hash server side.
 */
export async function publicSessionId(idHash: string): Promise<string> {
  return (await sha256Hex(idHash)).slice(0, 32);
}

// --- Revoking ---

export const DELETE_SESSION_SQL = "DELETE FROM sessions WHERE user_id = ? AND id_hash = ?";
export const DELETE_ALL_SESSIONS_SQL = "DELETE FROM sessions WHERE user_id = ?";
export const DELETE_ALL_SESSIONS_EXCEPT_SQL = "DELETE FROM sessions WHERE user_id = ? AND id_hash <> ?";
export const LIST_SESSION_HASHES_SQL = "SELECT id_hash FROM sessions WHERE user_id = ?";
export const LIST_SESSION_HASHES_EXCEPT_SQL = "SELECT id_hash FROM sessions WHERE user_id = ? AND id_hash <> ?";
export const LIST_DESKTOP_IDS_SQL = "SELECT id FROM desktops WHERE user_id = ? AND revoked_at IS NULL LIMIT ?";

/** The new session row is the proof that a conditional batch (login, registration, recovery) went through. */
export const SESSION_CREATED_SQL = "EXISTS (SELECT 1 FROM sessions WHERE id_hash = ?)";
export const SESSION_BY_HASH_SQL = "SELECT user_id FROM sessions WHERE id_hash = ?";

/**
 * DELETE of one session of a user, for a batch. The `user_id` condition makes it harmless for a foreign hash.
 * With `when` the row is deleted only if that SQL condition holds (for example: the new session was created).
 */
export function deleteSessionStatement(env: Pick<Env, "DB">, userId: string, idHash: string, when?: { sql: string; params: unknown[] }): D1PreparedStatement {
  if (!when) return env.DB.prepare(DELETE_SESSION_SQL).bind(userId, idHash);
  return env.DB.prepare(`${DELETE_SESSION_SQL} AND ${when.sql}`).bind(userId, idHash, ...when.params);
}

/**
 * The session row the request's cookie resolves to (any state: expired rows count, they are still rows), or
 * null. Login and registration delete it in the batch that creates the new session, so a browser that signs in
 * again does not leave an orphan row behind (it would sit in "Sessions" until it expires).
 */
export async function currentSessionHash(env: Pick<Env, "DB">, request: Request): Promise<{ userId: string; idHash: string } | null> {
  const token = readSessionToken(request);
  if (token === null) return null;
  const idHash = await sha256Hex(token);
  const row = await env.DB.prepare(SESSION_BY_HASH_SQL).bind(idHash).first<{ user_id: string }>();
  return row ? { userId: row.user_id, idHash } : null;
}

/** DELETE of every session of a user (except one, when given), for a batch such as the admin block. */
export function deleteAllSessionsStatement(env: Pick<Env, "DB">, userId: string, exceptIdHash?: string): D1PreparedStatement {
  return exceptIdHash === undefined
    ? env.DB.prepare(DELETE_ALL_SESSIONS_SQL).bind(userId)
    : env.DB.prepare(DELETE_ALL_SESSIONS_EXCEPT_SQL).bind(userId, exceptIdHash);
}

/**
 * The `id_hash` of every session of a user (except one). A caller that deletes with `deleteAllSessionsStatement`
 * inside its own batch lists first and calls `closeSessionSockets` after the batch commits.
 */
export async function listSessionIdHashes(env: Pick<Env, "DB">, userId: string, exceptIdHash?: string): Promise<string[]> {
  const stmt =
    exceptIdHash === undefined
      ? env.DB.prepare(LIST_SESSION_HASHES_SQL).bind(userId)
      : env.DB.prepare(LIST_SESSION_HASHES_EXCEPT_SQL).bind(userId, exceptIdHash);
  const { results } = await stmt.all<{ id_hash: string }>();
  return results.map((row) => row.id_hash);
}

function maxDesktops(env: Pick<Env, "MAX_DESKTOPS_PER_USER">): number {
  const parsed = Number.parseInt(env.MAX_DESKTOPS_PER_USER, 10);
  return Number.isInteger(parsed) && parsed > 0 ? parsed : 5;
}

/**
 * Closes the open sockets of revoked sessions: for each hash and each non-revoked desktop of the user (at most
 * `MAX_DESKTOPS_PER_USER`), `room.revoke({sessionIdHash}, 4001)` (architecture.md 4.4). Best effort: the session
 * rows are already gone, so a failing room is logged (no ids) and skipped. At most `MAX_REVOKE_CALLS` calls are
 * made, spent round-robin: session 0 on every desktop, then session 1 on every desktop, and so on, so a cut
 * never leaves one desktop without a call (recovery after a compromise must reach every desktop). A cut is
 * logged with counts only; the sessions beyond it keep an open socket until the relay re-checks them (WS-R
 * re-validates the cookie session on join). Pending WS-R contract change: `RevokeTarget` takes
 * `{ sessionIdHashes: string[] }`, which makes it one call per desktop and the cap moot.
 */
export async function closeSessionSockets(env: Pick<Env, "DB" | "ROOM" | "MAX_DESKTOPS_PER_USER">, userId: string, idHashes: readonly string[]): Promise<void> {
  if (idHashes.length === 0) return;
  const { results } = await env.DB.prepare(LIST_DESKTOP_IDS_SQL).bind(userId, maxDesktops(env)).all<{ id: string }>();
  const rooms = results.map((desktop) => env.ROOM.get(env.ROOM.idFromName(desktop.id)));
  const wanted = rooms.length * idHashes.length;
  const calls: Promise<void>[] = [];
  for (const sessionIdHash of idHashes) {
    for (const room of rooms) {
      if (calls.length >= MAX_REVOKE_CALLS) break;
      calls.push(room.revoke({ sessionIdHash }, CloseCodes.UNAUTHORIZED));
    }
  }
  if (wanted > calls.length) console.error(`room.revoke truncated: wanted=${wanted} calls=${calls.length} desktops=${rooms.length} sessions=${idHashes.length}`);
  const settled = await Promise.allSettled(calls);
  const failed = settled.filter((r) => r.status === "rejected").length;
  if (failed > 0) console.error(`room.revoke failed: calls=${calls.length} failed=${failed}`);
}

/**
 * Deletes the given sessions of a user (one batch, with `extra` statements such as an audit INSERT) and closes
 * their sockets. Returns the number of rows deleted. Used by logout and `DELETE /api/sessions/:id`. `extra` runs
 * right after the last DELETE, so an audit row conditioned on `PREVIOUS_CHANGED` depends on it (use it with one hash).
 */
export async function revokeSessions(
  env: Pick<Env, "DB" | "ROOM" | "MAX_DESKTOPS_PER_USER">,
  userId: string,
  idHashes: readonly string[],
  extra: readonly D1PreparedStatement[] = [],
): Promise<number> {
  const deletes = idHashes.map((idHash) => deleteSessionStatement(env, userId, idHash));
  const results = await env.DB.batch([...deletes, ...extra]);
  await closeSessionSockets(env, userId, idHashes);
  return results.slice(0, deletes.length).reduce((sum, r) => sum + (r.meta.changes ?? 0), 0);
}

/**
 * "Sign out everywhere": deletes every session of the user, except `exceptIdHash`, and closes their sockets.
 * Recovery (keep the new recovery session) and the admin block (keep none) call it, or batch
 * `deleteAllSessionsStatement` themselves and call `listSessionIdHashes` before and `closeSessionSockets` after.
 * Returns the number of sessions revoked.
 */
export async function revokeAllSessions(
  env: Pick<Env, "DB" | "ROOM" | "MAX_DESKTOPS_PER_USER">,
  userId: string,
  exceptIdHash?: string,
  extra: readonly D1PreparedStatement[] = [],
): Promise<number> {
  const hashes = await listSessionIdHashes(env, userId, exceptIdHash);
  const results = await env.DB.batch([deleteAllSessionsStatement(env, userId, exceptIdHash), ...extra]);
  await closeSessionSockets(env, userId, hashes);
  return results[0]?.meta.changes ?? hashes.length;
}
