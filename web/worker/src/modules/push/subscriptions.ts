// Push subscriptions and prefs (P-01): vapid-key, subscribe, unsubscribe and per-desktop prefs.
import type { Handler } from "hono";
import { PUSH_CATEGORIES, PushPrefsBody, PushPrefsResponse, PushPrefsStateResponse, PushSubscriptionBody, PushSubscriptionResponse, VapidKeyResponse } from "@monoptah/protocol";
import { apiError, respond, type AppEnv } from "../../http.ts";
import { nowSeconds } from "../../lib/time.ts";
import { requireAuth } from "../../middleware/auth.ts";
import { resolvePrefs } from "./prefs.ts";
import { fromBase64Url } from "./vapid.ts";

/** Push services a subscription endpoint may point to, so the Worker never POSTs to an arbitrary URL. */
export const PUSH_HOSTS_EXACT: readonly string[] = ["fcm.googleapis.com", "updates.push.services.mozilla.com", "web.push.apple.com"];
export const PUSH_HOST_SUFFIXES: readonly string[] = [".push.apple.com", ".notify.windows.com"];
export const MAX_ENDPOINT_LENGTH = 2048;
export const MAX_SUBSCRIPTIONS_PER_DEVICE = 4;

/** True for an https URL of at most 2048 characters on a known push service host (no credentials, default port). */
export function isAllowedEndpoint(endpoint: string): boolean {
  if (endpoint.length > MAX_ENDPOINT_LENGTH) return false;
  let url: URL;
  try {
    url = new URL(endpoint);
  } catch {
    return false;
  }
  if (url.protocol !== "https:" || url.username !== "" || url.password !== "" || url.port !== "") return false;
  const host = url.hostname.toLowerCase();
  return PUSH_HOSTS_EXACT.includes(host) || PUSH_HOST_SUFFIXES.some((suffix) => host.endsWith(suffix));
}

function decodedLength(value: string): { length: number; first: number } | null {
  try {
    const bytes = fromBase64Url(value);
    return { length: bytes.length, first: bytes[0] ?? -1 };
  } catch {
    return null;
  }
}

/** Insert, or move an existing endpoint to this user and device with new keys and failures reset. The count of the
 *  device excludes the same endpoint, so a re-subscribe never hits the quota. `changes = 0` means quota reached. */
export const UPSERT_SUBSCRIPTION_SQL = `INSERT INTO push_subscriptions (id, user_id, device_id, endpoint, p256dh, auth, vapid_key_id, ua, created_at, last_success_at, failures)
  SELECT ?, ?, ?, ?, ?, ?, ?, ?, ?, NULL, 0
  WHERE (SELECT count(*) FROM push_subscriptions WHERE device_id = ? AND endpoint <> ?) < ${MAX_SUBSCRIPTIONS_PER_DEVICE}
  ON CONFLICT (endpoint) DO UPDATE SET user_id = excluded.user_id, device_id = excluded.device_id, p256dh = excluded.p256dh, auth = excluded.auth,
    vapid_key_id = excluded.vapid_key_id, ua = excluded.ua, failures = 0`;
export const SUBSCRIPTION_ID_SQL = "SELECT id FROM push_subscriptions WHERE endpoint = ?";
export const OWN_DEVICE_SQL = "SELECT id FROM devices WHERE id = ? AND user_id = ? AND revoked_at IS NULL";
export const DELETE_SUBSCRIPTION_SQL = "DELETE FROM push_subscriptions WHERE id = ? AND user_id = ?";
export const OWN_DESKTOP_SQL = "SELECT id FROM desktops WHERE id = ? AND user_id = ? AND revoked_at IS NULL";
export const SELECT_PREFS_SQL = "SELECT enabled, categories FROM push_prefs WHERE user_id = ? AND desktop_id = ?";
export const UPSERT_PREFS_SQL = `INSERT INTO push_prefs (user_id, desktop_id, enabled, categories) VALUES (?, ?, ?, ?)
  ON CONFLICT (user_id, desktop_id) DO UPDATE SET enabled = excluded.enabled, categories = excluded.categories`;

/** `GET /api/push/vapid-key` (public). */
export const getVapidKey: Handler<AppEnv> = (c) => respond(c, VapidKeyResponse, { publicKey: c.env.VAPID_PUBLIC_KEY, keyId: c.env.VAPID_KEY_ID });

/** `POST /api/push/subscriptions`. */
export const subscribe: Handler<AppEnv> = async (c) => {
  const { user } = requireAuth(c);
  const parsed = PushSubscriptionBody.safeParse(await c.req.json().catch(() => null));
  if (!parsed.success) return apiError(c, 400, "invalid", "Invalid subscription");
  const body = parsed.data;
  if (!isAllowedEndpoint(body.endpoint)) return apiError(c, 400, "invalid", "Unsupported push endpoint");
  const p256dh = decodedLength(body.keys.p256dh);
  const auth = decodedLength(body.keys.auth);
  if (!p256dh || p256dh.length !== 65 || p256dh.first !== 4) return apiError(c, 400, "invalid", "Invalid p256dh key");
  if (!auth || auth.length !== 16) return apiError(c, 400, "invalid", "Invalid auth secret");
  if (body.vapidKeyId !== c.env.VAPID_KEY_ID) return apiError(c, 400, "invalid", "Stale VAPID key: subscribe again with the current key");
  const device = await c.env.DB.prepare(OWN_DEVICE_SQL).bind(body.deviceId, user.id).first();
  if (!device) return apiError(c, 404, "not_found", "Device not found");

  const id = crypto.randomUUID();
  const ua = (c.req.header("User-Agent") ?? "").slice(0, 200) || null;
  const [inserted, existing] = await c.env.DB.batch<{ id: string }>([
    c.env.DB.prepare(UPSERT_SUBSCRIPTION_SQL).bind(id, user.id, body.deviceId, body.endpoint, body.keys.p256dh, body.keys.auth, body.vapidKeyId, ua, nowSeconds(), body.deviceId, body.endpoint),
    c.env.DB.prepare(SUBSCRIPTION_ID_SQL).bind(body.endpoint),
  ]);
  if ((inserted?.meta.changes ?? 0) === 0) return apiError(c, 409, "quota_exceeded", "Too many subscriptions on this device");
  const stored = existing?.results[0]?.id;
  if (!stored) throw new Error("Stored subscription not found");
  return respond(c, PushSubscriptionResponse, { id: stored });
};

/** `DELETE /api/push/subscriptions/:id`: own subscriptions only. */
export const unsubscribe: Handler<AppEnv> = async (c) => {
  const { user } = requireAuth(c);
  const result = await c.env.DB.prepare(DELETE_SUBSCRIPTION_SQL).bind(c.req.param("id") ?? "", user.id).run();
  if (result.meta.changes === 0) return apiError(c, 404, "not_found", "Subscription not found");
  return c.json({ ok: true as const });
};

/** `GET /api/push/prefs/:desktopId`: owner of a non-revoked desktop. With no stored row the defaults (all on) come back with `stored: false`. */
export const getPrefs: Handler<AppEnv> = async (c) => {
  const { user } = requireAuth(c);
  const desktopId = c.req.param("desktopId") ?? "";
  const desktop = await c.env.DB.prepare(OWN_DESKTOP_SQL).bind(desktopId, user.id).first();
  if (!desktop) return apiError(c, 404, "not_found", "Desktop not found");
  const row = await c.env.DB.prepare(SELECT_PREFS_SQL).bind(user.id, desktopId).first<{ enabled: number; categories: string }>();
  return respond(c, PushPrefsStateResponse, resolvePrefs({ enabled: row?.enabled ?? null, categories: row?.categories ?? null }));
};

/** `PUT /api/push/prefs/:desktopId`: owner of a non-revoked desktop. Without a stored row the defaults (all on) apply. */
export const putPrefs: Handler<AppEnv> = async (c) => {
  const { user } = requireAuth(c);
  const parsed = PushPrefsBody.safeParse(await c.req.json().catch(() => null));
  if (!parsed.success) return apiError(c, 400, "invalid", "Invalid preferences");
  const desktopId = c.req.param("desktopId") ?? "";
  const desktop = await c.env.DB.prepare(OWN_DESKTOP_SQL).bind(desktopId, user.id).first();
  if (!desktop) return apiError(c, 404, "not_found", "Desktop not found");
  const categories = Object.fromEntries(PUSH_CATEGORIES.map((key) => [key, parsed.data.categories[key] === true])) as Record<(typeof PUSH_CATEGORIES)[number], boolean>;
  await c.env.DB.prepare(UPSERT_PREFS_SQL).bind(user.id, desktopId, parsed.data.enabled ? 1 : 0, JSON.stringify(categories)).run();
  return respond(c, PushPrefsResponse, { enabled: parsed.data.enabled, categories });
};
