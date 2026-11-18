// Handlers of the session routes: `GET /api/sessions`, `DELETE /api/sessions/:id`, `POST /api/auth/logout`.
import type { Handler } from "hono";
import { LogoutBody, SessionsResponse } from "@monoptah/protocol";
import { apiError, respond, type AppEnv } from "../../http.ts";
import { requireAuth } from "../../middleware/auth.ts";
import { nowSeconds } from "../../lib/time.ts";
import { auditStatement, PREVIOUS_CHANGED } from "../audit/audit.ts";
import { clearSessionCookie, publicSessionId, revokeSessions } from "./session.ts";

/** Live sessions of a user: not revoked, before both expiries. Uses `sessions_user_id_idx`. */
export const LIST_USER_SESSIONS_SQL =
  "SELECT id_hash, created_at, last_seen_at, user_agent, amr FROM sessions WHERE user_id = ? AND revoked_at IS NULL AND expires_at > ? AND idle_expires_at > ? ORDER BY created_at DESC";

/** The caller's push subscriptions on one device (index `push_subscriptions_device_id_idx`); `user_id` keeps it to their own. */
export const LOGOUT_PUSH_SQL = "DELETE FROM push_subscriptions WHERE device_id = ? AND user_id = ?";

interface ListRow {
  id_hash: string;
  created_at: number;
  last_seen_at: number;
  user_agent: string | null;
  amr: "passkey" | "recovery";
}

/** Own live sessions, newest first, with a `current` flag. Ids are the derived opaque ids (`publicSessionId`). */
export const listSessions: Handler<AppEnv> = async (c) => {
  const { user, session } = requireAuth(c);
  const now = nowSeconds();
  const { results } = await c.env.DB.prepare(LIST_USER_SESSIONS_SQL).bind(user.id, now, now).all<ListRow>();
  const sessions = await Promise.all(
    results.map(async (row) => ({
      id: await publicSessionId(row.id_hash),
      createdAt: row.created_at,
      lastSeenAt: row.last_seen_at,
      userAgent: row.user_agent,
      amr: row.amr,
      current: row.id_hash === session.idHash,
    })),
  );
  return respond(c, SessionsResponse, { sessions });
};

/**
 * Revokes one of the caller's own sessions: the row is deleted (so the next request with its cookie is 401) and
 * its sockets are closed through `room.revoke`. Another user's session and an unknown id are both 404. Revoking
 * the current session also clears the cookie.
 */
export const revokeSession: Handler<AppEnv> = async (c) => {
  const { user, session } = requireAuth(c);
  const id = c.req.param("id") ?? "";
  const now = nowSeconds();
  const { results } = await c.env.DB.prepare(LIST_USER_SESSIONS_SQL).bind(user.id, now, now).all<ListRow>();
  let target: ListRow | undefined;
  for (const row of results) {
    if ((await publicSessionId(row.id_hash)) === id) target = row;
  }
  if (!target) return apiError(c, 404, "not_found", "Session not found");
  // The audit row follows the one DELETE and is written only if that DELETE removed a row: a parallel revoke of the
  // same session that lost the race changes nothing and answers 404, like an unknown id.
  const revoked = await revokeSessions(c.env, user.id, [target.id_hash], [
    auditStatement(c.env, { action: "session.revoke", actorUserId: user.id, targetType: "session", targetId: id, meta: { all: false, count: 1 } }, PREVIOUS_CHANGED),
  ]);
  if (revoked === 0) return apiError(c, 404, "not_found", "Session not found");
  if (target.id_hash === session.idHash) c.header("Set-Cookie", clearSessionCookie(), { append: true });
  return c.json({ ok: true as const });
};

/** Deletes the current session row, closes its sockets and clears the cookie. */
export const logout: Handler<AppEnv> = async (c) => {
  const { user, session } = requireAuth(c);
  // The optional body names this browser's device. A missing or malformed body only skips the push cleanup:
  // signing out must never fail because of it.
  const body = LogoutBody.safeParse(await c.req.json().catch(() => undefined));
  // A recovery-only session never touches devices (recover.ts), so it gets no push cleanup either.
  const deviceId = body.success && !session.recoveryOnly ? body.data?.deviceId : undefined;
  // The push cleanup goes after the audit INSERT, which reads the changes() of the session DELETE before it.
  const pushCleanup = deviceId === undefined ? [] : [c.env.DB.prepare(LOGOUT_PUSH_SQL).bind(deviceId, user.id)];
  // Idempotent: a logout that finds its session already deleted (a parallel one) still clears the cookie, without an event.
  await revokeSessions(c.env, user.id, [session.idHash], [auditStatement(c.env, { action: "session.logout", actorUserId: user.id, meta: {} }, PREVIOUS_CHANGED), ...pushCleanup]);
  c.header("Set-Cookie", clearSessionCookie(), { append: true });
  return c.json({ ok: true as const });
};
