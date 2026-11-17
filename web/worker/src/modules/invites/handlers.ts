// Invite routes (A-06): create (quota, gate, code shown once), list and revoke. The registration side of the gate
// lives in modules/auth/register.ts and shares `gate.ts`.
import type { Handler } from "hono";
import { CreateInviteResponse, InvitesResponse } from "@monoptah/protocol";
import { apiError, respond, type AppEnv } from "../../http.ts";
import { nowSeconds } from "../../lib/time.ts";
import { requireAuth } from "../../middleware/auth.ts";
import { auditStatement } from "../audit/audit.ts";
import { INVITES_USED_SQL, ME_ROW_SQL } from "../me/handlers.ts";
import { hashInviteCode, INVITE_TTL_SECONDS, newInviteCode } from "./code.ts";
import { GATE_SQL, gateParams, registrationOpen } from "./gate.ts";

/** The user's own invites, newest first. Uses `invites_created_by_idx`; a user holds at most `invites_granted` rows. */
export const LIST_INVITES_SQL =
  "SELECT id, created_at, expires_at, used_at, revoked_at, used_by FROM invites WHERE created_by = ? AND kind = 'user' ORDER BY created_at DESC, id DESC";

/**
 * Creates an invite only while the creator has a slot and the gate is open, in one statement, so two parallel
 * creates at one remaining slot cannot both pass: the slot count (`invites_granted` minus invites that are not
 * revoked-and-unused, the same as `GET /api/me`) and the gate (`settings.invites_paused`, `MAX_USERS`, same
 * `GATE_SQL` as registration) are evaluated by the INSERT itself. `changes = 0` means no slot or a closed gate.
 */
export const INSERT_INVITE_SQL = `INSERT INTO invites (id, code_hash, created_by, kind, expires_at, created_at)
  SELECT ?, ?, u.id, 'user', ?, ? FROM users u
  WHERE u.id = ? AND (SELECT count(*) FROM invites WHERE created_by = u.id AND NOT (revoked_at IS NOT NULL AND used_at IS NULL)) < u.invites_granted
  AND ${GATE_SQL}`;
/** Looks an invite up by id (primary key) to tell "not yours" (404) from "already used or revoked" (409). */
export const INVITE_BY_ID_SQL = "SELECT id, created_by, kind, used_at, revoked_at FROM invites WHERE id = ?";
/** Revokes one unused, not yet revoked invite of its creator. */
export const REVOKE_INVITE_SQL = "UPDATE invites SET revoked_at = ? WHERE id = ? AND created_by = ? AND used_at IS NULL AND revoked_at IS NULL";
/** True only for the invite that the same batch just wrote (primary key lookup): conditions the audit row. */
export const INVITE_EXISTS_SQL = "EXISTS (SELECT 1 FROM invites WHERE id = ?)";
export const INVITE_REVOKED_NOW_SQL = "EXISTS (SELECT 1 FROM invites WHERE id = ? AND revoked_at = ?)";

interface InviteRow {
  id: string;
  created_at: number;
  expires_at: number;
  used_at: number | null;
  revoked_at: number | null;
  used_by: string | null;
}

function toInvite(row: InviteRow) {
  return { id: row.id, createdAt: row.created_at, expiresAt: row.expires_at, usedAt: row.used_at, revokedAt: row.revoked_at, usedBy: row.used_by };
}

/** `GET /api/invites`: own invites newest first, `granted`, and `remaining` (the same number as `invitesLeft` of `GET /api/me`). */
export const listInvites: Handler<AppEnv> = async (c) => {
  const { user } = requireAuth(c);
  const [list, row, used] = await Promise.all([
    c.env.DB.prepare(LIST_INVITES_SQL).bind(user.id).all<InviteRow>(),
    c.env.DB.prepare(ME_ROW_SQL).bind(user.id).first<{ invites_granted: number }>(),
    c.env.DB.prepare(INVITES_USED_SQL).bind(user.id).first<{ n: number }>(),
  ]);
  const granted = row?.invites_granted ?? 0;
  return respond(c, InvitesResponse, {
    invites: list.results.map(toInvite),
    granted,
    remaining: Math.max(0, granted - (used?.n ?? 0)),
  });
};

/**
 * `POST /api/invites`. The gate comes first: paused or `MAX_USERS` reached answers `409 invites_paused` (the user
 * can do nothing about it, so it is a state conflict and not a 403). Then the quota, atomically in the INSERT:
 * no slot left answers `409 quota_exceeded`. The code is 128 random bits, returned once; D1 keeps its SHA-256.
 * The audit row holds the invite id and never the code.
 */
export const createInvite: Handler<AppEnv> = async (c) => {
  const { user } = requireAuth(c);
  if (!(await registrationOpen(c.env))) return apiError(c, 409, "invites_paused", "Invites are paused");
  const now = nowSeconds();
  const id = crypto.randomUUID();
  const code = newInviteCode();
  const expiresAt = now + INVITE_TTL_SECONDS;
  const [inserted] = await c.env.DB.batch([
    c.env.DB.prepare(INSERT_INVITE_SQL).bind(id, await hashInviteCode(code), expiresAt, now, user.id, ...gateParams(c.env)),
    auditStatement(c.env, { action: "invite.create", actorUserId: user.id, targetType: "invite", targetId: id, meta: { inviteId: id } }, { sql: INVITE_EXISTS_SQL, params: [id] }),
  ]);
  if ((inserted?.meta.changes ?? 0) === 0) {
    // The gate may have closed between the early check and the INSERT: report that, else the quota.
    if (!(await registrationOpen(c.env))) return apiError(c, 409, "invites_paused", "Invites are paused");
    return apiError(c, 409, "quota_exceeded", "No invites left");
  }
  return respond(c, CreateInviteResponse, { invite: { id, createdAt: now, expiresAt, usedAt: null, revokedAt: null, usedBy: null }, code }, 201);
};

/**
 * `DELETE /api/invites/:id`: revokes one of the caller's own unused invites (the slot is given back). Another
 * user's invite, a bootstrap invite and an unknown id are all `404`; an invite that is used or already revoked is
 * `409 conflict`.
 */
export const revokeInvite: Handler<AppEnv> = async (c) => {
  const { user } = requireAuth(c);
  const id = c.req.param("id") ?? "";
  const invite = await c.env.DB.prepare(INVITE_BY_ID_SQL).bind(id).first<{ created_by: string | null; kind: string; used_at: number | null; revoked_at: number | null }>();
  if (!invite || invite.created_by !== user.id || invite.kind !== "user") return apiError(c, 404, "not_found", "Invite not found");
  if (invite.used_at !== null || invite.revoked_at !== null) return apiError(c, 409, "conflict", "Invite already used or revoked");
  const now = nowSeconds();
  const [updated] = await c.env.DB.batch([
    c.env.DB.prepare(REVOKE_INVITE_SQL).bind(now, id, user.id),
    auditStatement(c.env, { action: "invite.revoke", actorUserId: user.id, targetType: "invite", targetId: id, meta: { inviteId: id } }, { sql: INVITE_REVOKED_NOW_SQL, params: [id, now] }),
  ]);
  if ((updated?.meta.changes ?? 0) === 0) return apiError(c, 409, "conflict", "Invite already used or revoked");
  return c.json({ ok: true as const });
};
