// Relay upgrade routes (architecture.md 4.2 and 4.4). The Worker authenticates, then forwards the upgrade to the
// room of the desktop with internal headers. A refused upgrade is answered with a WebSocket that is accepted and
// closed at once, so the peer gets a close code it can act on (4001, 4003, 4011) instead of an opaque HTTP error.
import type { Handler } from "hono";
import { CloseCodes } from "@monoptah/protocol";
import { apiError, type AppContext, type AppEnv } from "../../http.ts";
import { requireAuth } from "../../middleware/auth.ts";
import { roomOf } from "../../relay/rooms.ts";
import { defineModule, routeKey } from "../define.ts";
import { consumeTokenLimit, resolveDesktopToken, type TokenRefusal } from "../desktops/token.ts";

/**
 * One indexed lookup (primary key of `device_grants`): the device belongs to the session user and is not revoked,
 * the desktop belongs to the same user and is not revoked, and the grant is not `revoked` (`pending` may join).
 * Binds: device id, desktop id, user id, user id.
 */
export const DEVICE_JOIN_SQL =
  "SELECT g.state AS state FROM device_grants g JOIN devices v ON v.id = g.device_id JOIN desktops d ON d.id = g.desktop_id WHERE g.device_id = ? AND g.desktop_id = ? AND g.state != 'revoked' AND v.user_id = ? AND v.revoked_at IS NULL AND d.user_id = ? AND d.revoked_at IS NULL";

/** Headers only the Worker may set; client copies are removed before forwarding. */
const INTERNAL_HEADERS = ["x-role", "x-desktop-id", "x-device-id", "x-user-id", "x-session-id-hash", "x-session-expires-at", "x-grant-state"];

const UUID_RE = /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/i;

const REFUSAL_CODE: Record<TokenRefusal, number> = {
  missing: CloseCodes.UNAUTHORIZED,
  invalid: CloseCodes.UNAUTHORIZED,
  scope: CloseCodes.UNAUTHORIZED,
  revoked: CloseCodes.REVOKED,
  blocked: CloseCodes.BLOCKED,
};

/** A 101 whose socket is accepted and closed with `code` at once. */
function refuse(code: number): Response {
  const pair = new WebSocketPair();
  const server = pair[1];
  server.accept();
  server.close(code, "refused");
  return new Response(null, { status: 101, webSocket: pair[0] });
}

function forward(c: AppContext, desktopId: string, internal: Record<string, string>): Promise<Response> {
  const headers = new Headers(c.req.raw.headers);
  for (const name of INTERNAL_HEADERS) headers.delete(name);
  headers.delete("Cookie");
  headers.delete("Authorization");
  headers.set("x-desktop-id", desktopId);
  for (const [name, value] of Object.entries(internal)) headers.set(name, value);
  return roomOf(c.env, desktopId).fetch(new Request(c.req.url, { headers }));
}

function isUpgrade(c: AppContext): boolean {
  return c.req.header("Upgrade")?.toLowerCase() === "websocket";
}

const desktopUpgrade: Handler<AppEnv> = async (c) => {
  if (!isUpgrade(c)) return apiError(c, 426, "invalid", "WebSocket upgrade required");
  const desktopId = c.req.param("desktopId") ?? "";
  const limited = await consumeTokenLimit(c.req.raw, "relay");
  if (limited && !limited.result.ok) return refuse(CloseCodes.RATE_LIMITED);
  const resolved = await resolveDesktopToken(c.env, c.req.raw, "relay", undefined, limited?.hash);
  if (!resolved.ok) return refuse(REFUSAL_CODE[resolved.reason]);
  if (resolved.principal.desktopId !== desktopId) return refuse(CloseCodes.UNAUTHORIZED);
  return forward(c, desktopId, { "x-role": "desktop", "x-user-id": resolved.principal.userId });
};

const deviceUpgrade: Handler<AppEnv> = async (c) => {
  if (!isUpgrade(c)) return apiError(c, 426, "invalid", "WebSocket upgrade required");
  const desktopId = c.req.param("desktopId") ?? "";
  const deviceId = c.req.query("device") ?? "";
  const { user, session } = requireAuth(c);
  if (!UUID_RE.test(deviceId)) return refuse(CloseCodes.REVOKED);
  const row = await c.env.DB.prepare(DEVICE_JOIN_SQL).bind(deviceId, desktopId, user.id, user.id).first<{ state: "pending" | "confirmed" }>();
  if (!row) return refuse(CloseCodes.REVOKED);
  return forward(c, desktopId, { "x-role": "device", "x-device-id": deviceId, "x-user-id": user.id, "x-session-id-hash": session.idHash, "x-session-expires-at": String(session.expiresAt), "x-grant-state": row.state });
};

export const routes = defineModule("relay", {
  [routeKey({ method: "GET", path: "/relay/desktop/:desktopId" })]: desktopUpgrade,
  [routeKey({ method: "GET", path: "/relay/device/:desktopId" })]: deviceUpgrade,
});
