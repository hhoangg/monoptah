// Admin routes (A-09, security.md 3.6 and 4). Every route needs `role = 'admin'` and a fresh session: the auth
// resolver answers 403 `forbidden` or `fresh_required` before a handler here runs.
import type { Handler } from "hono";
import {
  AdminAuditResponse,
  AdminBlockBody,
  AdminBlockResponse,
  AdminGrantInvitesBody,
  AdminPutSettingsBody,
  AdminSettingsSchema,
  AdminUsersResponse,
  CloseCodes,
  PageQuery,
} from "@monoptah/protocol";
import type { Env } from "../../env.ts";
import { apiError, respond, type AppEnv } from "../../http.ts";
import { nowSeconds } from "../../lib/time.ts";
import { requireAuth } from "../../middleware/auth.ts";
import { auditStatement, auditStatementCounted, PREVIOUS_CHANGED } from "../audit/audit.ts";
import { decodeCursor, DEFAULT_PAGE_SIZE, encodeCursor } from "../audit/handlers.ts";
import { INVITES_PAUSED_SQL, maxUsers, USER_COUNT_SQL } from "../invites/gate.ts";
import {
  blockUsersSql,
  deletePushSubscriptionsSql,
  deleteSessionsSql,
  listDesktopsSql,
  revokeDesktopGrantsSql,
  revokeDeviceGrantsSql,
  revokeInvitesSql,
  revokeTokensSql,
} from "./branch.ts";

/** Total invites one user can be granted (`invites_granted`), so repeated grants cannot grow without bound. */
export const MAX_INVITES_GRANTED = 1000;

/** Every user, oldest first. One query: `desktopCount` is a correlated subquery on `desktops_user_id_idx`. */
export const LIST_USERS_SQL =
  "SELECT u.id, u.handle, u.display_name, u.role, u.status, u.invited_by, u.blocked_via, u.invites_granted, u.created_at, (SELECT count(*) FROM desktops d WHERE d.user_id = u.id AND d.revoked_at IS NULL) AS desktop_count FROM users u ORDER BY u.created_at, u.id";
export const USER_STATUS_SQL = "SELECT status FROM users WHERE id = ?";
export const UNBLOCK_SQL = "UPDATE users SET status = 'active', blocked_at = NULL, blocked_reason = NULL, blocked_via = NULL WHERE id = ? AND status = 'blocked'";
export const GRANT_ALLOWED_SQL = "EXISTS (SELECT 1 FROM users WHERE id = ? AND invites_granted + ? <= ?)";
export const GRANT_INVITES_SQL = "UPDATE users SET invites_granted = invites_granted + ? WHERE id = ? AND invites_granted + ? <= ?";
export const UPSERT_SETTING_SQL =
  "INSERT INTO settings (key, value, updated_at, updated_by) VALUES ('invites_paused', ?, ?, ?) ON CONFLICT(key) DO UPDATE SET value = excluded.value, updated_at = excluded.updated_at, updated_by = excluded.updated_by";
/** First page of every event, newest first. Uses `audit_log_ts_id_idx` (migration 0003). */
export const ADMIN_AUDIT_PAGE_SQL =
  "SELECT id, ts, actor_user_id, actor_kind, action, target_type, target_id, meta FROM audit_log ORDER BY ts DESC, id DESC LIMIT ?";
export const ADMIN_AUDIT_PAGE_AFTER_SQL =
  "SELECT id, ts, actor_user_id, actor_kind, action, target_type, target_id, meta FROM audit_log WHERE (ts, id) < (?, ?) ORDER BY ts DESC, id DESC LIMIT ?";

interface AdminUserRow {
  id: string;
  handle: string;
  display_name: string;
  role: "user" | "admin";
  status: "active" | "blocked";
  invited_by: string | null;
  blocked_via: string | null;
  invites_granted: number;
  created_at: number;
  desktop_count: number;
}

/** `GET /api/admin/users`: every user (blocked ones too) by creation time, with the live desktop count. */
export const listUsers: Handler<AppEnv> = async (c) => {
  const { results } = await c.env.DB.prepare(LIST_USERS_SQL).all<AdminUserRow>();
  return respond(c, AdminUsersResponse, {
    users: results.map((row) => ({
      id: row.id,
      handle: row.handle,
      displayName: row.display_name,
      role: row.role,
      status: row.status,
      invitedBy: row.invited_by,
      blockedVia: row.blocked_via,
      invitesGranted: row.invites_granted,
      desktopCount: row.desktop_count,
      createdAt: row.created_at,
    })),
  });
};

/**
 * `POST /api/admin/users/:id/block {cascade, reason}`. One `db.batch` (one transaction) blocks the branch (the target,
 * plus every descendant of the invite tree with `cascade`), deletes the sessions of the members, revokes their desktop
 * tokens and the grants on their desktops and of their devices, deletes their push subscriptions, revokes their
 * unused invites (used ones keep their row), and writes one `user.block` audit event for the root
 * (`{cascade, count}`, written only if the block changed a row, `count` computed in the batch; the free-text reason lives only in `users.blocked_reason`, security.md 10).
 *
 * `blocked_via`: with `cascade`, the root id on every member including the root (the admin UI finds "the branch of X"
 * by it, and unblocking the root clears only the root's own value); without `cascade`, NULL. A member that is already
 * blocked is left as it was. The acting admin is never a member: when the admin sits inside the branch, the admin
 * is skipped and the admin's own descendants are still blocked. Blocking yourself is `409 conflict`, an unknown id `404`.
 *
 * After the commit, `room.revoke({all: true}, 4011)` runs on the live desktops of the members (at most
 * `MAX_REVOKE_CALLS`, the subrequest cap). `all` covers every socket of those rooms, so no per-session call is
 * needed. A member's device that is connected to someone else's desktop is not closed here; the relay re-checks the
 * user `status` and the session when a socket joins (WS-R), and the sessions are already deleted. Best effort: a
 * failing room is logged without ids and skipped, the block itself has committed. The room is a stub until R-02.
 */
export const blockUser: Handler<AppEnv> = async (c) => {
  const { user: admin } = requireAuth(c);
  const body = AdminBlockBody.safeParse(await c.req.json().catch(() => null));
  if (!body.success) return apiError(c, 400, "invalid", "Invalid block request");
  const targetId = c.req.param("id") ?? "";
  if (targetId === admin.id) return apiError(c, 409, "conflict", "You cannot block yourself");
  const target = await c.env.DB.prepare(USER_STATUS_SQL).bind(targetId).first<{ status: string }>();
  if (!target) return apiError(c, 404, "not_found", "User not found");

  const { cascade, reason } = body.data;
  const db = c.env.DB;
  const desktops = await db.prepare(listDesktopsSql(cascade)).bind(targetId, admin.id).all<{ id: string }>();
  const now = nowSeconds();
  const results = await db.batch([
    db.prepare(blockUsersSql(cascade)).bind(targetId, now, reason, cascade ? targetId : null, admin.id),
    // Right after the block: written only if the block changed a row (a parallel block that lost the race leaves
    // no event), with `count` = the rows it changed.
    auditStatementCounted(c.env, { action: "user.block", actorUserId: admin.id, targetType: "user", targetId, meta: { cascade, count: 0 } }),
    db.prepare(deleteSessionsSql(cascade)).bind(targetId, admin.id),
    db.prepare(revokeTokensSql(cascade)).bind(targetId, now, admin.id),
    db.prepare(revokeDesktopGrantsSql(cascade)).bind(targetId, now, admin.id),
    db.prepare(revokeDeviceGrantsSql(cascade)).bind(targetId, now, admin.id),
    db.prepare(deletePushSubscriptionsSql(cascade)).bind(targetId, admin.id),
    db.prepare(revokeInvitesSql(cascade)).bind(targetId, now, admin.id),
  ]);

  const calls = desktops.results.map((desktop) => c.env.ROOM.get(c.env.ROOM.idFromName(desktop.id)).revoke({ all: true }, CloseCodes.BLOCKED));
  const settled = await Promise.allSettled(calls);
  const failed = settled.filter((r) => r.status === "rejected").length;
  if (failed > 0) console.error(`room.revoke failed on block: calls=${calls.length} failed=${failed}`);

  return respond(c, AdminBlockResponse, { blocked: results[0]?.meta.changes ?? 0 });
};

/**
 * `POST /api/admin/users/:id/unblock`: restores sign-in only (`status`, `blocked_at`, `blocked_reason`, `blocked_via`).
 * It never cascades and never touches tokens, grants, push subscriptions or invites (security.md 3.6). A user that is
 * not blocked is `409 conflict`, an unknown id `404`.
 */
export const unblockUser: Handler<AppEnv> = async (c) => {
  const { user: admin } = requireAuth(c);
  const targetId = c.req.param("id") ?? "";
  const target = await c.env.DB.prepare(USER_STATUS_SQL).bind(targetId).first<{ status: string }>();
  if (!target) return apiError(c, 404, "not_found", "User not found");
  if (target.status !== "blocked") return apiError(c, 409, "conflict", "User is not blocked");
  const [updated] = await c.env.DB.batch([
    c.env.DB.prepare(UNBLOCK_SQL).bind(targetId),
    auditStatement(c.env, { action: "user.unblock", actorUserId: admin.id, targetType: "user", targetId, meta: { cascade: false } }, PREVIOUS_CHANGED),
  ]);
  if ((updated?.meta.changes ?? 0) === 0) return apiError(c, 409, "conflict", "User is not blocked");
  return c.json({ ok: true as const });
};

/** `POST /api/admin/users/:id/invites {count}`: adds to `invites_granted`, at most `MAX_INVITES_GRANTED` in total. */
export const grantInvites: Handler<AppEnv> = async (c) => {
  const { user: admin } = requireAuth(c);
  const body = AdminGrantInvitesBody.safeParse(await c.req.json().catch(() => null));
  if (!body.success) return apiError(c, 400, "invalid", "Invalid invite count");
  const targetId = c.req.param("id") ?? "";
  const { count } = body.data;
  const target = await c.env.DB.prepare(USER_STATUS_SQL).bind(targetId).first<{ status: string }>();
  if (!target) return apiError(c, 404, "not_found", "User not found");
  // The audit row goes first: it and the UPDATE evaluate the same condition on the same state.
  const [, updated] = await c.env.DB.batch([
    auditStatement(c.env, { action: "admin.invites_grant", actorUserId: admin.id, targetType: "user", targetId, meta: { count } }, { sql: GRANT_ALLOWED_SQL, params: [targetId, count, MAX_INVITES_GRANTED] }),
    c.env.DB.prepare(GRANT_INVITES_SQL).bind(count, targetId, count, MAX_INVITES_GRANTED),
  ]);
  if ((updated?.meta.changes ?? 0) === 0) return apiError(c, 409, "conflict", `A user can hold at most ${MAX_INVITES_GRANTED} invites`);
  return c.json({ ok: true as const });
};

async function readSettings(env: Pick<Env, "DB" | "MAX_USERS">) {
  const [paused, count] = await Promise.all([
    env.DB.prepare(INVITES_PAUSED_SQL).first<{ value: string }>(),
    env.DB.prepare(USER_COUNT_SQL).first<{ n: number }>(),
  ]);
  const userCount = count?.n ?? 0;
  return { invitesPaused: paused?.value === "1", maxUsers: maxUsers(env, userCount), userCount };
}

/** `GET /api/admin/settings`: the pause switch, `MAX_USERS` (0 = no cap) and the user count (blocked ones included). */
export const getSettings: Handler<AppEnv> = async (c) => respond(c, AdminSettingsSchema, await readSettings(c.env));

/** `PUT /api/admin/settings {invitesPaused}`: upserts the `invites_paused` row and its audit event in one batch. */
export const putSettings: Handler<AppEnv> = async (c) => {
  const { user: admin } = requireAuth(c);
  const body = AdminPutSettingsBody.safeParse(await c.req.json().catch(() => null));
  if (!body.success) return apiError(c, 400, "invalid", "Invalid settings");
  const { invitesPaused } = body.data;
  await c.env.DB.batch([
    c.env.DB.prepare(UPSERT_SETTING_SQL).bind(invitesPaused ? "1" : "0", nowSeconds(), admin.id),
    auditStatement(c.env, { action: "admin.settings_change", actorUserId: admin.id, targetType: "setting", targetId: "invites_paused", meta: { invitesPaused } }),
  ]);
  return respond(c, AdminSettingsSchema, await readSettings(c.env));
};

interface AdminAuditRow {
  id: string;
  ts: number;
  actor_user_id: string | null;
  actor_kind: "user" | "desktop" | "admin" | "system";
  action: string;
  target_type: string | null;
  target_id: string | null;
  meta: string;
}

/** `GET /api/admin/audit`: every event, newest first, paged by the `(ts, id)` cursor of `GET /api/audit`. */
export const listAudit: Handler<AppEnv> = async (c) => {
  const query = PageQuery.safeParse(c.req.query());
  if (!query.success) return apiError(c, 400, "invalid", "Invalid page query");
  const limit = query.data.limit ?? DEFAULT_PAGE_SIZE;
  let stmt: D1PreparedStatement;
  if (query.data.cursor === undefined) {
    stmt = c.env.DB.prepare(ADMIN_AUDIT_PAGE_SQL).bind(limit + 1);
  } else {
    const cursor = decodeCursor(query.data.cursor);
    if (!cursor) return apiError(c, 400, "invalid", "Invalid cursor");
    stmt = c.env.DB.prepare(ADMIN_AUDIT_PAGE_AFTER_SQL).bind(cursor.ts, cursor.id, limit + 1);
  }
  const { results } = await stmt.all<AdminAuditRow>();
  const page = results.slice(0, limit);
  const last = page[page.length - 1];
  return respond(c, AdminAuditResponse, {
    events: page.map((row) => ({
      id: row.id,
      ts: row.ts,
      action: row.action,
      targetType: row.target_type,
      targetId: row.target_id,
      meta: JSON.parse(row.meta) as Record<string, unknown>,
      actorUserId: row.actor_user_id,
      actorKind: row.actor_kind,
    })),
    nextCursor: results.length > limit && last ? encodeCursor(last.ts, last.id) : null,
  });
};
