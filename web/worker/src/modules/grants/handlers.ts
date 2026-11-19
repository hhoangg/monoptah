// Grant routes (R-04, R-15 server part). State machine `pending -> confirmed -> revoked`. The desktop is the
// authority for `confirmed` and `permission`: only `POST /api/desktop/grants/:deviceId/state` (desktop token) sets
// them. The web side can request access (`pending`), ask for a permission (`requested_permission`) and revoke.
import type { Handler } from "hono";
import {
  DesktopGrantStateBody,
  DesktopSelectionTicketBody,
  SelectionTicketResponse,
  DesktopGrantsResponse,
  GrantResponse,
  GrantsResponse,
  PutGrantBody,
  RequestGrantBody,
} from "@monoptah/protocol";
import { apiError, respond, type AppContext, type AppEnv } from "../../http.ts";
import { randomToken, sha256Hex } from "../../lib/crypto.ts";
import { identifierHash } from "../../lib/ip.ts";
import { nowSeconds } from "../../lib/time.ts";
import { requireAuth, requireFresh } from "../../middleware/auth.ts";
import { notifyGrants, revokeInRooms, setGrantStateInRooms } from "../../relay/rooms.ts";
import { auditStatement, PREVIOUS_CHANGED } from "../audit/audit.ts";
import { auditBreach, consume, LIMITS, rateLimitKey } from "../ratelimit/index.ts";
import { requireDesktopToken } from "../desktops/token.ts";

const REVOKED = 4003;

/** An own, live desktop: the owner check of every `/api/desktops/:id/grants` route (primary key). */
export const OWN_DESKTOP_SQL = "SELECT id FROM desktops WHERE id = ? AND user_id = ? AND revoked_at IS NULL";
export const OWN_DEVICE_SQL = "SELECT id FROM devices WHERE id = ? AND user_id = ? AND revoked_at IS NULL";
const GRANT_COLUMNS =
  "g.device_id, dv.name, dv.fingerprint, CASE WHEN dv.revoked_at IS NULL THEN g.state ELSE 'revoked' END AS state, g.permission, g.requested_permission, g.confirmed_at, g.updated_at";
/** One grant with its device (primary keys). */
export const GET_GRANT_SQL = `SELECT ${GRANT_COLUMNS} FROM device_grants g JOIN devices dv ON dv.id = g.device_id WHERE g.desktop_id = ? AND g.device_id = ?`;
/** The grants of a desktop on devices that were not forgotten (index `device_grants_desktop_id_idx`). */
export const LIST_GRANTS_SQL = `SELECT ${GRANT_COLUMNS} FROM device_grants g JOIN devices dv ON dv.id = g.device_id WHERE g.desktop_id = ? AND dv.revoked_at IS NULL ORDER BY g.updated_at, g.device_id`;
/** Creates the pending grant when none exists and both ends are the caller's and live. A conflict changes nothing. */
export const INSERT_GRANT_SQL = `INSERT INTO device_grants (device_id, desktop_id, state, updated_at)
  SELECT ?, ?, 'pending', ? WHERE EXISTS (SELECT 1 FROM devices WHERE id = ? AND user_id = ? AND revoked_at IS NULL)
  AND EXISTS (SELECT 1 FROM desktops WHERE id = ? AND user_id = ? AND revoked_at IS NULL) ON CONFLICT DO NOTHING`;
export const REQUEST_PERMISSION_SQL = "UPDATE device_grants SET requested_permission = ?, updated_at = ? WHERE device_id = ? AND desktop_id = ? AND state != 'revoked'";
export const REVOKE_GRANT_SQL = "UPDATE device_grants SET state = 'revoked', updated_at = ? WHERE device_id = ? AND desktop_id = ? AND state != 'revoked'";
/** Every grant of this desktop; a grant of a forgotten device reads as `revoked`. */
export const DESKTOP_GRANTS_SQL =
  "SELECT g.device_id, dv.name, dv.public_key, dv.fingerprint, CASE WHEN dv.revoked_at IS NULL THEN g.state ELSE 'revoked' END AS state, g.requested_permission, g.permission FROM device_grants g JOIN devices dv ON dv.id = g.device_id WHERE g.desktop_id = ?";
/** What the desktop reports about one of its grants: effective state (a forgotten device is revoked). */
export const DESKTOP_GRANT_STATE_SQL =
  "SELECT CASE WHEN dv.revoked_at IS NULL THEN g.state ELSE 'revoked' END AS state, g.permission, g.requested_permission FROM device_grants g JOIN devices dv ON dv.id = g.device_id WHERE g.desktop_id = ? AND g.device_id = ?";
/** Desktop report `confirmed`: sets permission and `confirmed_at`; a matching request is fulfilled. */
export const CONFIRM_GRANT_SQL = `UPDATE device_grants SET state = 'confirmed', permission = ?1, confirmed_at = coalesce(confirmed_at, ?2), updated_at = ?2,
  requested_permission = CASE WHEN requested_permission = ?1 THEN NULL ELSE requested_permission END
  WHERE device_id = ?3 AND desktop_id = ?4 AND state != 'revoked'`;
/** Desktop report `pending`: back to waiting, permission cleared. */
export const PENDING_GRANT_SQL = "UPDATE device_grants SET state = 'pending', permission = NULL, confirmed_at = NULL, updated_at = ? WHERE device_id = ? AND desktop_id = ? AND state != 'revoked'";

interface GrantRow {
  device_id: string;
  name: string;
  fingerprint: string;
  state: "pending" | "confirmed" | "revoked";
  permission: "read" | "control" | null;
  requested_permission: "read" | "control" | null;
  confirmed_at: number | null;
  updated_at: number;
}

function toGrant(row: GrantRow) {
  return {
    deviceId: row.device_id,
    deviceName: row.name,
    fingerprint: row.fingerprint,
    state: row.state,
    permission: row.permission,
    requestedPermission: row.requested_permission,
    confirmedAt: row.confirmed_at,
    updatedAt: row.updated_at,
  };
}

const notFound = (c: AppContext, what = "Desktop"): Response => apiError(c, 404, "not_found", `${what} not found`);

async function ownsDesktop(c: AppContext, desktopId: string, userId: string): Promise<boolean> {
  return (await c.env.DB.prepare(OWN_DESKTOP_SQL).bind(desktopId, userId).first()) !== null;
}

/**
 * `POST /api/desktops/:id/grants {deviceId}`: a device asks for access. Creates `pending` when there is no grant;
 * an existing `pending` or `confirmed` one is returned unchanged; a `revoked` one is 409 (a revoked device comes
 * back only as a new device). The desktop is told to re-read its grants.
 */
export const requestGrant: Handler<AppEnv> = async (c) => {
  const { user } = requireAuth(c);
  const desktopId = c.req.param("id") ?? "";
  const body = RequestGrantBody.safeParse(await c.req.json().catch(() => null));
  if (!body.success) return apiError(c, 400, "invalid", "Invalid grant request");
  const { deviceId } = body.data;
  if (!(await ownsDesktop(c, desktopId, user.id))) return notFound(c);
  if ((await c.env.DB.prepare(OWN_DEVICE_SQL).bind(deviceId, user.id).first()) === null) return notFound(c, "Device");

  const [inserted] = await c.env.DB.batch([
    c.env.DB.prepare(INSERT_GRANT_SQL).bind(deviceId, desktopId, nowSeconds(), deviceId, user.id, desktopId, user.id),
    auditStatement(c.env, { action: "grant.change", actorUserId: user.id, targetType: "grant", targetId: deviceId, meta: { deviceId, desktopId, state: "pending" } }, PREVIOUS_CHANGED),
  ]);
  const row = await c.env.DB.prepare(GET_GRANT_SQL).bind(desktopId, deviceId).first<GrantRow>();
  if (!row) return notFound(c);
  if (row.state === "revoked") return apiError(c, 409, "conflict", "Grant was revoked; register the device again");
  const created = inserted?.meta.changes === 1;
  if (created) await notifyGrants(c.env, desktopId);
  return respond(c, GrantResponse, { grant: toGrant(row) }, created ? 201 : 200);
};

/** `GET /api/desktops/:id/grants`. */
export const listGrants: Handler<AppEnv> = async (c) => {
  const { user } = requireAuth(c);
  const desktopId = c.req.param("id") ?? "";
  if (!(await ownsDesktop(c, desktopId, user.id))) return notFound(c);
  const list = await c.env.DB.prepare(LIST_GRANTS_SQL).bind(desktopId).all<GrantRow>();
  return respond(c, GrantsResponse, { grants: list.results.map(toGrant) });
};

/**
 * `PUT /api/desktops/:id/grants/:deviceId`. `requestedPermission` only records a request (the desktop applies it);
 * the server never writes `permission` or `confirmed`. `state: "revoked"` needs a fresh session and closes the
 * device's sockets (4003) at once; it wins when both fields are sent. An empty body is 400.
 */
export const putGrant: Handler<AppEnv> = async (c) => {
  const { user } = requireAuth(c);
  const desktopId = c.req.param("id") ?? "";
  const deviceId = c.req.param("deviceId") ?? "";
  const body = PutGrantBody.safeParse(await c.req.json().catch(() => null));
  if (!body.success || (body.data.requestedPermission === undefined && body.data.state === undefined)) return apiError(c, 400, "invalid", "Invalid grant update");
  if (body.data.state === "revoked") {
    const stale = requireFresh(c);
    if (stale) return stale;
  }
  if (!(await ownsDesktop(c, desktopId, user.id))) return notFound(c);
  const before = await c.env.DB.prepare(GET_GRANT_SQL).bind(desktopId, deviceId).first<GrantRow>();
  if (!before) return notFound(c, "Grant");

  const now = nowSeconds();
  if (body.data.state === "revoked") {
    const [revoked] = await c.env.DB.batch([
      c.env.DB.prepare(REVOKE_GRANT_SQL).bind(now, deviceId, desktopId),
      auditStatement(c.env, { action: "grant.change", actorUserId: user.id, targetType: "grant", targetId: deviceId, meta: { deviceId, desktopId, state: "revoked" } }, PREVIOUS_CHANGED),
    ]);
    // Also when D1 already shows the revocation: the room call of the first request may have failed (idempotent, R13B-1).
    if (revoked?.meta.changes === 1 || before.state === "revoked") await revokeInRooms(c.env, [desktopId], { deviceId }, REVOKED);
  } else {
    if (before.state === "revoked") return apiError(c, 409, "conflict", "Grant is revoked");
    const requested = body.data.requestedPermission as "read" | "control";
    const [updated] = await c.env.DB.batch([
      c.env.DB.prepare(REQUEST_PERMISSION_SQL).bind(requested, now, deviceId, desktopId),
      auditStatement(c.env, { action: "grant.change", actorUserId: user.id, targetType: "grant", targetId: deviceId, meta: { deviceId, desktopId, requestedPermission: requested } }, PREVIOUS_CHANGED),
    ]);
    if (updated?.meta.changes === 0) return apiError(c, 409, "conflict", "Grant is revoked");
  }
  await notifyGrants(c.env, desktopId);
  const after = await c.env.DB.prepare(GET_GRANT_SQL).bind(desktopId, deviceId).first<GrantRow>();
  return after ? respond(c, GrantResponse, { grant: toGrant(after) }) : notFound(c, "Grant");
};

/** `GET /api/desktop/grants` (token `grants`): every grant of this desktop with the device key and name (R-15). */
export const desktopGrants: Handler<AppEnv> = async (c) => {
  const auth = await requireDesktopToken(c, "grants");
  if (auth instanceof Response) return auth;
  const list = await c.env.DB.prepare(DESKTOP_GRANTS_SQL).bind(auth.desktopId).all<{
    device_id: string;
    name: string;
    public_key: string;
    fingerprint: string;
    state: "pending" | "confirmed" | "revoked";
    requested_permission: "read" | "control" | null;
    permission: "read" | "control" | null;
  }>();
  return respond(c, DesktopGrantsResponse, {
    grants: list.results.map((r) => ({
      deviceId: r.device_id,
      deviceName: r.name,
      publicKey: r.public_key,
      fingerprint: r.fingerprint,
      state: r.state,
      requestedPermission: r.requested_permission,
      permission: r.permission,
    })),
  });
};

/**
 * `POST /api/desktop/grants/:deviceId/state` (token `grants`): the only route that sets `confirmed` and
 * `permission`. A grant the server revoked cannot be resurrected (409); reporting `revoked` for it is accepted.
 * A reported `revoked` closes that device's sockets (4003).
 */
export const desktopGrantState: Handler<AppEnv> = async (c) => {
  const auth = await requireDesktopToken(c, "grants");
  if (auth instanceof Response) return auth;
  const deviceId = c.req.param("deviceId") ?? "";
  const body = DesktopGrantStateBody.safeParse(await c.req.json().catch(() => null));
  if (!body.success) return apiError(c, 400, "invalid", "Invalid grant state");
  const { state, permission } = body.data;
  if (state === "confirmed" && permission === undefined) return apiError(c, 400, "invalid", "permission is required for confirmed");

  const current = await c.env.DB.prepare(DESKTOP_GRANT_STATE_SQL).bind(auth.desktopId, deviceId).first<{ state: string; permission: string | null; requested_permission: string | null }>();
  if (!current) return notFound(c, "Grant");
  if (current.state === "revoked") {
    // Idempotent: make sure the room has closed that device, no D1 write.
    if (state === "revoked") {
      await revokeInRooms(c.env, [auth.desktopId], { deviceId }, REVOKED);
      return c.json({ ok: true as const });
    }
    return apiError(c, 409, "conflict", "Grant is revoked on the server");
  }
  // Unchanged report: no UPDATE, no audit row (a desktop re-reporting every grant would burn the write budget),
  // unless a `confirmed` report would clear a stale request for the permission already held.
  const clearsRequest = state === "confirmed" && current.requested_permission === permission;
  if (state === current.state && (state !== "confirmed" || permission === current.permission) && !clearsRequest) return c.json({ ok: true as const });

  const now = nowSeconds();
  const update =
    state === "confirmed"
      ? c.env.DB.prepare(CONFIRM_GRANT_SQL).bind(permission, now, deviceId, auth.desktopId)
      : state === "pending"
        ? c.env.DB.prepare(PENDING_GRANT_SQL).bind(now, deviceId, auth.desktopId)
        : c.env.DB.prepare(REVOKE_GRANT_SQL).bind(now, deviceId, auth.desktopId);
  const meta = state === "confirmed" ? { deviceId, desktopId: auth.desktopId, state, permission: permission as "read" | "control" } : { deviceId, desktopId: auth.desktopId, state };
  const [changed] = await c.env.DB.batch([
    update,
    auditStatement(c.env, { action: "grant.change", actorKind: "desktop", actorUserId: auth.userId, targetType: "grant", targetId: deviceId, meta }, PREVIOUS_CHANGED),
  ]);
  if (changed?.meta.changes === 0) return apiError(c, 409, "conflict", "Grant is revoked on the server");
  if (state === "revoked") await revokeInRooms(c.env, [auth.desktopId], { deviceId }, REVOKED);
  else await setGrantStateInRooms(c.env, auth.desktopId, deviceId, state);
  return c.json({ ok: true as const });
};

/** Lifetime of a selection ticket, seconds. The desktop verifies it right after the web sends it. */
export const SELECTION_TICKET_TTL_SECONDS = 120;
/** The device must belong to the caller and not be revoked, and its grant on this desktop must be confirmed with Control. */
export const CONTROL_GRANT_SQL =
  "SELECT 1 AS ok FROM device_grants g JOIN devices dv ON dv.id = g.device_id WHERE g.desktop_id = ? AND g.device_id = ? AND dv.user_id = ? AND dv.revoked_at IS NULL AND g.state = 'confirmed' AND g.permission = 'control'";
export const INSERT_SELECTION_TICKET_SQL =
  "INSERT INTO selection_tickets (token_hash, user_id, desktop_id, device_id, created_at, expires_at) VALUES (?, ?, ?, ?, ?, ?)";
/** Single use: only one caller changes the row, and only for the desktop and device it was made for. */
export const CONSUME_SELECTION_TICKET_SQL =
  "UPDATE selection_tickets SET used_at = ?1 WHERE token_hash = ?2 AND desktop_id = ?3 AND device_id = ?4 AND used_at IS NULL AND expires_at >= ?1 AND EXISTS (SELECT 1 FROM device_grants g JOIN devices dv ON dv.id = g.device_id WHERE g.desktop_id = selection_tickets.desktop_id AND g.device_id = selection_tickets.device_id AND dv.revoked_at IS NULL AND g.state = 'confirmed' AND g.permission = 'control')";
/**
 * How recent the passkey re-auth must be to mint a ticket, seconds. The general "fresh" window is 5 minutes, which
 * would let a hijacked tab mint right after any other re-auth; a minute makes the web prompt for the passkey just
 * before widening.
 */
export const SELECTION_TICKET_REAUTH_SECONDS = 60;

/**
 * `POST /api/desktops/:id/grants/:deviceId/selection-ticket` (fresh session): a single-use ticket that proves a
 * passkey re-auth to the desktop, so a Control web device can widen its selection. Only the SHA-256 is stored.
 * 10 per user per hour.
 */
export const createSelectionTicket: Handler<AppEnv> = async (c) => {
  const { user, session } = requireAuth(c);
  const desktopId = c.req.param("id") ?? "";
  const deviceId = c.req.param("deviceId") ?? "";
  if (session.reauthAt === null || session.reauthAt < nowSeconds() - SELECTION_TICKET_REAUTH_SECONDS) return apiError(c, 403, "fresh_required", "Re-authentication required");
  if (!(await ownsDesktop(c, desktopId, user.id))) return notFound(c);
  if ((await c.env.DB.prepare(CONTROL_GRANT_SQL).bind(desktopId, deviceId, user.id).first()) === null) return apiError(c, 403, "forbidden", "This device does not have Control access to this desktop");

  const limit = await consume(c.env, rateLimitKey("selection-ticket", "user", await identifierHash(c.env, "user", user.id)), LIMITS["selection-ticket"].limit, LIMITS["selection-ticket"].windowSeconds);
  if (!limit.ok) {
    if (limit.firstBreach) await auditBreach(c, "selection-ticket", LIMITS["selection-ticket"], user.id);
    c.header("Retry-After", String(limit.retryAfter));
    return apiError(c, 429, "rate_limited", "Too many requests");
  }

  const ticket = randomToken(32);
  const now = nowSeconds();
  const expiresAt = now + SELECTION_TICKET_TTL_SECONDS;
  await c.env.DB.batch([
    c.env.DB.prepare(INSERT_SELECTION_TICKET_SQL).bind(await sha256Hex(ticket), user.id, desktopId, deviceId, now, expiresAt),
    auditStatement(c.env, { action: "selection.ticket", actorUserId: user.id, targetType: "grant", targetId: deviceId, meta: { deviceId, desktopId } }),
  ]);
  return respond(c, SelectionTicketResponse, { ticket, expiresAt });
};

/**
 * `POST /api/desktop/grants/:deviceId/selection-ticket` (token `grants`): the desktop asks whether a ticket the web
 * sent is valid for this desktop and device, and consumes it. 200 once; a missing, expired, used or foreign ticket is 404.
 */
export const consumeSelectionTicket: Handler<AppEnv> = async (c) => {
  const auth = await requireDesktopToken(c, "grants");
  if (auth instanceof Response) return auth;
  const deviceId = c.req.param("deviceId") ?? "";
  const body = DesktopSelectionTicketBody.safeParse(await c.req.json().catch(() => null));
  if (!body.success) return apiError(c, 400, "invalid", "Invalid ticket");
  const now = nowSeconds();
  const used = await c.env.DB.prepare(CONSUME_SELECTION_TICKET_SQL).bind(now, await sha256Hex(body.data.ticket), auth.desktopId, deviceId).run();
  if (used.meta.changes !== 1) return apiError(c, 404, "not_found", "Ticket not found");
  await auditStatement(c.env, { action: "selection.widen", actorKind: "desktop", actorUserId: auth.userId, targetType: "grant", targetId: deviceId, meta: { deviceId, desktopId: auth.desktopId } }).run();
  return c.json({ ok: true as const });
};
