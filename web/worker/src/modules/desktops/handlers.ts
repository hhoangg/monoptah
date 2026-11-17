// Desktop routes (R-03, R-14a): the user's directory of desktops, and the token routes the desktop itself calls.
import type { Handler } from "hono";
import {
  DesktopDirectoryResponse,
  DesktopMeResponse,
  DesktopsResponse,
  PatchDesktopBody,
  PatchDesktopResponse,
} from "@monoptah/protocol";
import { apiError, respond, type AppContext, type AppEnv } from "../../http.ts";
import { nowSeconds } from "../../lib/time.ts";
import { requireAuth } from "../../middleware/auth.ts";
import { presenceOf, revokeInRooms } from "../../relay/rooms.ts";
import { auditStatement, PREVIOUS_CHANGED } from "../audit/audit.ts";
import { requireDesktopToken } from "./token.ts";

const REVOKED = 4003;

const DESKTOP_COLUMNS = "id, name, color, os, app_version, last_seen_at";
/** The user's live desktops (index `desktops_user_id_idx`; a user holds at most `MAX_DESKTOPS_PER_USER`). */
export const LIST_DESKTOPS_SQL = `SELECT ${DESKTOP_COLUMNS} FROM desktops WHERE user_id = ? AND revoked_at IS NULL ORDER BY created_at, id`;
/** Pending grants of non-revoked devices per desktop, one grouped query. */
export const PENDING_COUNTS_SQL =
  "SELECT g.desktop_id AS desktop_id, COUNT(*) AS n FROM desktops d JOIN device_grants g ON g.desktop_id = d.id JOIN devices dv ON dv.id = g.device_id WHERE d.user_id = ? AND d.revoked_at IS NULL AND g.state = 'pending' AND dv.revoked_at IS NULL GROUP BY g.desktop_id";
export const PENDING_COUNT_ONE_SQL =
  "SELECT COUNT(*) AS n FROM device_grants g JOIN devices dv ON dv.id = g.device_id WHERE g.desktop_id = ? AND g.state = 'pending' AND dv.revoked_at IS NULL";
/** One desktop of the user, with the key the device pins. */
export const GET_DESKTOP_SQL = `SELECT ${DESKTOP_COLUMNS}, public_key, fingerprint, user_id FROM desktops WHERE id = ? AND revoked_at IS NULL`;
/** Rename and recolor; `color` only changes when the flag is 1 (null clears it). */
export const PATCH_DESKTOP_SQL =
  "UPDATE desktops SET name = coalesce(?, name), color = CASE WHEN ? = 1 THEN ? ELSE color END WHERE id = ? AND user_id = ? AND revoked_at IS NULL";
/** Condition of the revoke batch: the desktop is the user's and still live. */
const LIVE_OWN = "EXISTS (SELECT 1 FROM desktops WHERE id = ? AND user_id = ? AND revoked_at IS NULL)";
export const REVOKE_DESKTOP_TOKENS_SQL = `UPDATE desktop_tokens SET revoked_at = ? WHERE desktop_id = ? AND revoked_at IS NULL AND ${LIVE_OWN}`;
export const REVOKE_DESKTOP_GRANTS_SQL = `UPDATE device_grants SET state = 'revoked', updated_at = ? WHERE desktop_id = ? AND state != 'revoked' AND ${LIVE_OWN}`;
export const REVOKE_DESKTOP_SQL = "UPDATE desktops SET revoked_at = ? WHERE id = ? AND user_id = ? AND revoked_at IS NULL";
export const DESKTOP_ME_SQL = "SELECT d.id, d.name, u.handle FROM desktops d JOIN users u ON u.id = d.user_id WHERE d.id = ?";
/** Denies the approved, not yet consumed pairing requests of a desktop (they could still mint a token). Index `pairing_requests_desktop_id_idx`. */
export const UNPAIR_DENY_APPROVED_SQL = "UPDATE pairing_requests SET status = 'denied' WHERE desktop_id = ? AND status = 'approved'";
/** Same for the delete batch, only when the desktop is the caller's and this batch just revoked it. Binds: desktop id, desktop id, user id, now. */
export const DELETE_DENY_APPROVED_SQL =
  "UPDATE pairing_requests SET status = 'denied' WHERE desktop_id = ? AND status = 'approved' AND EXISTS (SELECT 1 FROM desktops WHERE id = ? AND user_id = ? AND revoked_at = ?)";
/** Unpair: every token of the desktop; the desktop row stays so a later re-pair can replace it. */
export const UNPAIR_TOKENS_SQL = "UPDATE desktop_tokens SET revoked_at = ? WHERE desktop_id = ? AND revoked_at IS NULL";

interface DesktopRow {
  id: string;
  name: string;
  color: string | null;
  os: string | null;
  app_version: string | null;
  last_seen_at: number | null;
  public_key?: string;
  fingerprint?: string;
  user_id?: string;
}

function summaryOf(row: DesktopRow, online: boolean, pendingCount: number) {
  return { id: row.id, name: row.name, color: row.color, online, pendingCount, os: row.os, appVersion: row.app_version, lastSeenAt: row.last_seen_at };
}

const notFound = (c: AppContext): Response => apiError(c, 404, "not_found", "Desktop not found");

/** `GET /api/desktops`: live desktops with presence (parallel DO calls) and pending counts. */
export const listDesktops: Handler<AppEnv> = async (c) => {
  const { user } = requireAuth(c);
  const [list, counts] = await Promise.all([
    c.env.DB.prepare(LIST_DESKTOPS_SQL).bind(user.id).all<DesktopRow>(),
    c.env.DB.prepare(PENDING_COUNTS_SQL).bind(user.id).all<{ desktop_id: string; n: number }>(),
  ]);
  const pending = new Map(counts.results.map((r) => [r.desktop_id, r.n]));
  const online = await presenceOf(c.env, list.results.map((r) => r.id));
  return respond(c, DesktopsResponse, { desktops: list.results.map((r) => summaryOf(r, online.get(r.id) ?? false, pending.get(r.id) ?? 0)) });
};

/** One desktop of the caller, with online and pending count. Null for another user's or a revoked desktop. */
async function ownedSummary(c: AppContext, userId: string, id: string): Promise<{ row: DesktopRow; online: boolean; pending: number } | null> {
  const row = await c.env.DB.prepare(GET_DESKTOP_SQL).bind(id).first<DesktopRow>();
  if (!row || row.user_id !== userId) return null;
  const [count, online] = await Promise.all([c.env.DB.prepare(PENDING_COUNT_ONE_SQL).bind(id).first<{ n: number }>(), presenceOf(c.env, [id])]);
  return { row, online: online.get(id) ?? false, pending: count?.n ?? 0 };
}

/** `GET /api/desktops/:id`: the directory entry a device pins (public key and full fingerprint). */
export const getDesktop: Handler<AppEnv> = async (c) => {
  const { user } = requireAuth(c);
  const found = await ownedSummary(c, user.id, c.req.param("id") ?? "");
  if (!found) return notFound(c);
  return respond(c, DesktopDirectoryResponse, {
    ...summaryOf(found.row, found.online, found.pending),
    publicKey: found.row.public_key ?? "",
    fingerprint: found.row.fingerprint ?? "",
  });
};

/** `PATCH /api/desktops/:id`: name and color. An empty body is 400. */
export const patchDesktop: Handler<AppEnv> = async (c) => {
  const { user } = requireAuth(c);
  const id = c.req.param("id") ?? "";
  const body = PatchDesktopBody.safeParse(await c.req.json().catch(() => null));
  if (!body.success || (body.data.name === undefined && body.data.color === undefined)) return apiError(c, 400, "invalid", "Invalid desktop update");
  const colorGiven = body.data.color !== undefined;
  const result = await c.env.DB.prepare(PATCH_DESKTOP_SQL).bind(body.data.name ?? null, colorGiven ? 1 : 0, body.data.color ?? null, id, user.id).run();
  if (result.meta.changes === 0) return notFound(c);
  const found = await ownedSummary(c, user.id, id);
  if (!found) return notFound(c);
  return respond(c, PatchDesktopResponse, { desktop: summaryOf(found.row, found.online, found.pending) });
};

/**
 * `DELETE /api/desktops/:id` (fresh). One batch: the dependents first (tokens, grants), while the desktop is still
 * live and the caller's, then the desktop itself and the audit row. Then the room closes every socket (4003).
 */
export const deleteDesktop: Handler<AppEnv> = async (c) => {
  const { user } = requireAuth(c);
  const id = c.req.param("id") ?? "";
  const now = nowSeconds();
  const results = await c.env.DB.batch([
    c.env.DB.prepare(REVOKE_DESKTOP_TOKENS_SQL).bind(now, id, id, user.id),
    c.env.DB.prepare(REVOKE_DESKTOP_GRANTS_SQL).bind(now, id, id, user.id),
    c.env.DB.prepare(REVOKE_DESKTOP_SQL).bind(now, id, user.id),
    auditStatement(c.env, { action: "desktop.revoke", actorUserId: user.id, targetType: "desktop", targetId: id, meta: { desktopId: id } }, PREVIOUS_CHANGED),
    // After the audit row, which reads `changes()` of the revoke above.
    c.env.DB.prepare(DELETE_DENY_APPROVED_SQL).bind(id, id, user.id, now),
  ]);
  if (results[2]?.meta.changes !== 1) return notFound(c);
  await revokeInRooms(c.env, [id], { all: true }, REVOKED);
  return c.json({ ok: true as const });
};

/** `GET /api/desktop/me` (token `relay`): the desktop's identity and the account handle. */
export const desktopMe: Handler<AppEnv> = async (c) => {
  const auth = await requireDesktopToken(c, "relay");
  if (auth instanceof Response) return auth;
  const row = await c.env.DB.prepare(DESKTOP_ME_SQL).bind(auth.desktopId).first<{ id: string; name: string; handle: string }>();
  if (!row) return notFound(c);
  return respond(c, DesktopMeResponse, { desktopId: row.id, name: row.name, account: { handle: row.handle } });
};

/**
 * `DELETE /api/desktop/me` (token `relay`, Unpair): revokes every token of the desktop and closes its sockets.
 * The desktop row and its grants stay, so a later pairing can replace it and no orphan is created.
 */
export const unpairDesktop: Handler<AppEnv> = async (c) => {
  const auth = await requireDesktopToken(c, "relay");
  if (auth instanceof Response) return auth;
  const id = auth.desktopId;
  await c.env.DB.batch([
    c.env.DB.prepare(UNPAIR_TOKENS_SQL).bind(nowSeconds(), id),
    auditStatement(c.env, { action: "desktop.revoke", actorKind: "desktop", actorUserId: auth.userId, targetType: "desktop", targetId: id, meta: { desktopId: id } }, PREVIOUS_CHANGED),
    c.env.DB.prepare(UNPAIR_DENY_APPROVED_SQL).bind(id),
  ]);
  await revokeInRooms(c.env, [id], { all: true }, REVOKED);
  return c.json({ ok: true as const });
};
