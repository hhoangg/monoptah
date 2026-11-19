// Device routes (R-04): a browser profile registers its public key, lists, renames and forgets devices.
// Forgetting revokes every grant of the device and closes its sockets on each desktop it had access to.
import type { Handler } from "hono";
import { DevicesResponse, PatchDeviceBody, PatchDeviceResponse, RegisterDeviceBody, RegisterDeviceResponse } from "@monoptah/protocol";
import { apiError, respond, type AppContext, type AppEnv } from "../../http.ts";
import { nowSeconds } from "../../lib/time.ts";
import { requireAuth } from "../../middleware/auth.ts";
import { notifyGrants, revokeInRooms } from "../../relay/rooms.ts";
import { auditStatement, PREVIOUS_CHANGED } from "../audit/audit.ts";
import { maxDevicesPerUser } from "../desktops/limits.ts";
import { decodePublicKey, fingerprintOf } from "./keys.ts";

const REVOKED = 4003;

const DEVICE_COLUMNS = "id, name, fingerprint, created_at, last_seen_at";
/** The user's live device with this key (index `devices_user_id_idx`; at most `MAX_DEVICES_PER_USER` rows). */
export const FIND_DEVICE_SQL = "SELECT id FROM devices WHERE user_id = ? AND fingerprint = ? AND revoked_at IS NULL";
/** Registers a device unless the same key is already live (idempotent) or the user is at the quota, in one statement. */
export const INSERT_DEVICE_SQL = `INSERT INTO devices (id, user_id, name, public_key, fingerprint, created_at)
  SELECT ?, ?, ?, ?, ?, ? WHERE (SELECT COUNT(*) FROM devices WHERE user_id = ? AND revoked_at IS NULL) < ?
  AND NOT EXISTS (SELECT 1 FROM devices WHERE user_id = ? AND fingerprint = ? AND revoked_at IS NULL)`;
export const LIST_DEVICES_SQL = `SELECT ${DEVICE_COLUMNS} FROM devices WHERE user_id = ? AND revoked_at IS NULL ORDER BY created_at, id`;
export const PATCH_DEVICE_SQL = `UPDATE devices SET name = ? WHERE id = ? AND user_id = ? AND revoked_at IS NULL RETURNING ${DEVICE_COLUMNS}`;
/** The grants of a device, for the forget batch and the rooms to tell (primary key prefix). */
export const DEVICE_GRANTS_SQL = "SELECT desktop_id, state FROM device_grants WHERE device_id = ?";
const LIVE_OWN = "EXISTS (SELECT 1 FROM devices WHERE id = ? AND user_id = ? AND revoked_at IS NULL)";
export const FORGET_GRANTS_SQL = `UPDATE device_grants SET state = 'revoked', updated_at = ? WHERE device_id = ? AND state != 'revoked' AND ${LIVE_OWN}`;
export const FORGET_PUSH_SQL = `DELETE FROM push_subscriptions WHERE device_id = ? AND ${LIVE_OWN}`;
export const FORGOTTEN_OWN_SQL = "SELECT 1 FROM devices WHERE id = ? AND user_id = ? AND revoked_at IS NOT NULL";
export const FORGET_DEVICE_SQL = "UPDATE devices SET revoked_at = ? WHERE id = ? AND user_id = ? AND revoked_at IS NULL";

interface DeviceRow {
  id: string;
  name: string;
  fingerprint: string;
  created_at: number;
  last_seen_at: number | null;
}

function toDevice(row: DeviceRow) {
  return { id: row.id, name: row.name, fingerprint: row.fingerprint, createdAt: row.created_at, lastSeenAt: row.last_seen_at };
}

const notFound = (c: AppContext): Response => apiError(c, 404, "not_found", "Device not found");

/**
 * `POST /api/devices`: a live device with the same key is returned as is (200). Otherwise the INSERT checks the
 * quota itself, so parallel registrations cannot pass it together: no row written means `409 quota_exceeded`.
 */
export const registerDevice: Handler<AppEnv> = async (c) => {
  const { user } = requireAuth(c);
  const body = RegisterDeviceBody.safeParse(await c.req.json().catch(() => null));
  if (!body.success) return apiError(c, 400, "invalid", "Invalid device");
  const key = decodePublicKey(body.data.publicKey);
  if (key === null) return apiError(c, 400, "invalid", "publicKey must be 32 bytes, base64url");
  const fingerprint = await fingerprintOf(key);

  const existing = await c.env.DB.prepare(FIND_DEVICE_SQL).bind(user.id, fingerprint).first<{ id: string }>();
  if (existing) return respond(c, RegisterDeviceResponse, { deviceId: existing.id });

  const id = crypto.randomUUID();
  const [inserted] = await c.env.DB.batch([
    c.env.DB.prepare(INSERT_DEVICE_SQL).bind(id, user.id, body.data.name, body.data.publicKey, fingerprint, nowSeconds(), user.id, maxDevicesPerUser(c.env), user.id, fingerprint),
    auditStatement(c.env, { action: "device.create", actorUserId: user.id, targetType: "device", targetId: id, meta: { deviceId: id } }, PREVIOUS_CHANGED),
  ]);
  if (inserted?.meta.changes === 1) return respond(c, RegisterDeviceResponse, { deviceId: id }, 201);
  // Not written: a parallel call registered the same key (return it), else the quota is reached.
  const raced = await c.env.DB.prepare(FIND_DEVICE_SQL).bind(user.id, fingerprint).first<{ id: string }>();
  if (raced) return respond(c, RegisterDeviceResponse, { deviceId: raced.id });
  return apiError(c, 409, "quota_exceeded", "Device limit reached");
};

/** `GET /api/devices`: the user's devices that were not forgotten. */
export const listDevices: Handler<AppEnv> = async (c) => {
  const { user } = requireAuth(c);
  const list = await c.env.DB.prepare(LIST_DEVICES_SQL).bind(user.id).all<DeviceRow>();
  return respond(c, DevicesResponse, { devices: list.results.map(toDevice) });
};

/** `PATCH /api/devices/:id`: rename. */
export const patchDevice: Handler<AppEnv> = async (c) => {
  const { user } = requireAuth(c);
  const body = PatchDeviceBody.safeParse(await c.req.json().catch(() => null));
  if (!body.success) return apiError(c, 400, "invalid", "Invalid device name");
  const row = await c.env.DB.prepare(PATCH_DEVICE_SQL).bind(body.data.name, c.req.param("id") ?? "", user.id).first<DeviceRow>();
  if (!row) return notFound(c);
  return respond(c, PatchDeviceResponse, { device: toDevice(row) });
};

/**
 * `DELETE /api/devices/:id` (forget). One batch while the device is still live and the caller's: its grants become
 * `revoked`, its push subscriptions go, then the device is revoked and audited. Afterwards each desktop it had a
 * grant on closes that device's sockets (4003) and re-reads its grants.
 */
export const forgetDevice: Handler<AppEnv> = async (c) => {
  const { user } = requireAuth(c);
  const id = c.req.param("id") ?? "";
  const grants = await c.env.DB.prepare(DEVICE_GRANTS_SQL).bind(id).all<{ desktop_id: string; state: string }>();
  const now = nowSeconds();
  const results = await c.env.DB.batch([
    c.env.DB.prepare(FORGET_GRANTS_SQL).bind(now, id, id, user.id),
    c.env.DB.prepare(FORGET_PUSH_SQL).bind(id, id, user.id),
    c.env.DB.prepare(FORGET_DEVICE_SQL).bind(now, id, user.id),
    auditStatement(
      c.env,
      { action: "device.revoke", actorUserId: user.id, targetType: "device", targetId: id, meta: { deviceId: id, grants: grants.results.filter((g) => g.state !== "revoked").length } },
      PREVIOUS_CHANGED,
    ),
  ]);
  const desktopIds = grants.results.map((g) => g.desktop_id);
  if (results[2]?.meta.changes !== 1) {
    // Already forgotten by this user: tell the rooms again, the first call may have failed (idempotent, R13B-1).
    const forgotten = await c.env.DB.prepare(FORGOTTEN_OWN_SQL).bind(id, user.id).first();
    if (forgotten) await revokeInRooms(c.env, desktopIds, { deviceId: id }, REVOKED);
    return notFound(c);
  }
  await revokeInRooms(c.env, desktopIds, { deviceId: id }, REVOKED);
  await Promise.all(desktopIds.map((desktopId) => notifyGrants(c.env, desktopId)));
  return c.json({ ok: true as const });
};
