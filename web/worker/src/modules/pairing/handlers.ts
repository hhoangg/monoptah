// Pairing routes (R-03, architecture.md 6.4 and 6.7). The desktop starts a device authorization, a signed-in
// user approves or denies it on the web, and the desktop collects its token with `poll`. The token is minted
// in `poll`, once, and never stored in plain text.
import type { Handler } from "hono";
import {
  PAIRING_POLL_INTERVAL_SECONDS,
  PAIRING_TTL_SECONDS,
  PairApproveBody,
  PairApproveResponse,
  PairInfoResponse,
  PairPollBody,
  PairPollResponse,
  PairStartBody,
  PairStartResponse,
} from "@monoptah/protocol";
import { apiError, respond, type AppContext, type AppEnv } from "../../http.ts";
import { randomToken, sha256Hex, toBase64Url } from "../../lib/crypto.ts";
import { clientIp, ipHash } from "../../lib/ip.ts";
import { nowSeconds } from "../../lib/time.ts";
import { requireAuth } from "../../middleware/auth.ts";
import { revokeInRooms } from "../../relay/rooms.ts";
import { auditStatement, PREVIOUS_CHANGED } from "../audit/audit.ts";
import { TOKEN_SCOPES } from "../desktops/token.ts";
import { maxDesktopsPerUser } from "../desktops/limits.ts";
import { decodeBase64Url, decodePublicKey, fingerprintOfStored } from "../devices/keys.ts";
import { limitOutstandingPairings } from "../ratelimit/index.ts";
import { formatUserCode, newConfirmationCode, newUserCode, normalizeUserCode } from "./codes.ts";

/** A poll within this many seconds of the previous one is answered `slow_down`. */
export const SLOW_DOWN_SECONDS = 2;
/** How often `pair/start` retries the rare `user_code` collision. */
const START_ATTEMPTS = 5;
/** Close code of a revoked socket (protocol `CloseCodes.REVOKED`). */
const REVOKED = 4003;

/** Unique `user_code` and `device_code_hash`: a collision changes no row. */
export const INSERT_PAIRING_SQL = `INSERT INTO pairing_requests (id, user_code, device_code_hash, challenge, desktop_public_key, machine_name, os, confirmation_code, status, created_at, expires_at, ip_hash)
  VALUES (?, ?, ?, ?, ?, ?, ?, ?, 'pending', ?, ?, ?) ON CONFLICT DO NOTHING`;
/** Poll lookup by the device code hash (unique index), with the approving account's handle. */
export const POLL_LOOKUP_SQL =
  "SELECT p.id, p.challenge, p.status, p.expires_at, p.desktop_id, u.handle FROM pairing_requests p LEFT JOIN users u ON u.id = p.user_id WHERE p.device_code_hash = ?";
/** Records a poll unless the previous one was within `SLOW_DOWN_SECONDS`: 0 changes means `slow_down`. */
export const POLL_TOUCH_SQL = "UPDATE pairing_requests SET last_polled_at = ? WHERE id = ? AND (last_polled_at IS NULL OR last_polled_at <= ?)";
/** First half of the token mint: only the poll that flips `approved` to `consumed` goes on. */
export const CONSUME_PAIRING_SQL = "UPDATE pairing_requests SET status = 'consumed' WHERE id = ? AND status = 'approved'";
/** An approved request can still be collected this many seconds after `expires_at` (an approve in the last seconds). */
export const PAIRING_COLLECT_GRACE_SECONDS = 60;
/**
 * Second half: the token row (hash only), written only when the consume changed a row, the desktop is live, its key
 * is still the one the request carries (a later replace swaps it) and the collection grace has not run out.
 * Binds: token id, token hash, scope, now, request id, desktop id, now.
 */
export const INSERT_TOKEN_SQL = `INSERT INTO desktop_tokens (id, desktop_id, token_hash, scope, created_at)
  SELECT ?, d.id, ?, ?, ? FROM desktops d JOIN pairing_requests p ON p.id = ?
  WHERE d.id = ? AND d.revoked_at IS NULL AND d.public_key = p.desktop_public_key AND p.expires_at + ${PAIRING_COLLECT_GRACE_SECONDS} > ? AND changes() > 0`;
/** Request by user code (unique index). */
export const PAIRING_BY_CODE_SQL =
  "SELECT id, machine_name, os, desktop_public_key, confirmation_code, status, expires_at FROM pairing_requests WHERE user_code = ?";
/** A new desktop only for a pending, unexpired request and while the account is under its quota. */
export const INSERT_DESKTOP_SQL = `INSERT INTO desktops (id, user_id, name, public_key, fingerprint, os, created_at)
  SELECT ?, ?, p.machine_name, p.desktop_public_key, ?, p.os, ? FROM pairing_requests p
  WHERE p.id = ? AND p.status = 'pending' AND p.expires_at > ?
  AND (SELECT COUNT(*) FROM desktops WHERE user_id = ? AND revoked_at IS NULL) < ?`;
/** Binds the request to the user and the new desktop; the desktop row must already exist (foreign key). */
export const APPROVE_NEW_SQL =
  "UPDATE pairing_requests SET status = 'approved', user_id = ?, desktop_id = ? WHERE id = ? AND status = 'pending' AND EXISTS (SELECT 1 FROM desktops WHERE id = ?)";
/** The desktop a replace targets: the user's own and not revoked. */
export const REPLACE_TARGET_SQL = "SELECT id, name FROM desktops WHERE id = ? AND user_id = ? AND revoked_at IS NULL";
/** Condition of every statement of a replace: the request is pending and unexpired, the desktop is the user's. */
const REPLACE_GUARD =
  "EXISTS (SELECT 1 FROM pairing_requests WHERE id = ? AND status = 'pending' AND expires_at > ?) AND EXISTS (SELECT 1 FROM desktops WHERE id = ? AND user_id = ? AND revoked_at IS NULL)";
/** Swaps the key of the desktop; id, name and color stay. */
export const REPLACE_KEY_SQL = `UPDATE desktops SET public_key = (SELECT desktop_public_key FROM pairing_requests WHERE id = ?), fingerprint = ?, os = (SELECT os FROM pairing_requests WHERE id = ?) WHERE id = ? AND ${REPLACE_GUARD}`;
export const REPLACE_REVOKE_TOKENS_SQL = `UPDATE desktop_tokens SET revoked_at = ? WHERE desktop_id = ? AND revoked_at IS NULL AND ${REPLACE_GUARD}`;
/** Every non-revoked grant goes back to `pending`; revoked ones stay revoked. */
export const REPLACE_RESET_GRANTS_SQL = `UPDATE device_grants SET state = 'pending', permission = NULL, requested_permission = NULL, confirmed_at = NULL, updated_at = ? WHERE desktop_id = ? AND state != 'revoked' AND ${REPLACE_GUARD}`;
export const APPROVE_REPLACE_SQL =
  "UPDATE pairing_requests SET status = 'approved', user_id = ?, desktop_id = ? WHERE id = ? AND status = 'pending' AND expires_at > ? AND EXISTS (SELECT 1 FROM desktops WHERE id = ? AND user_id = ? AND revoked_at IS NULL)";
/**
 * After an approve with `replaces` committed: every other approved, not yet consumed request of the desktop is
 * denied, so an older approval cannot mint a token for the new key. Only when this request is the approved one.
 * Index `pairing_requests_desktop_id_idx`. Binds: desktop id, request id, request id, desktop id.
 */
export const DENY_OTHER_APPROVED_SQL =
  "UPDATE pairing_requests SET status = 'denied' WHERE desktop_id = ? AND status = 'approved' AND id != ? AND EXISTS (SELECT 1 FROM pairing_requests WHERE id = ? AND status = 'approved' AND desktop_id = ?)";
export const DENY_PAIRING_SQL = "UPDATE pairing_requests SET status = 'denied', user_id = ? WHERE user_code = ? AND status = 'pending' AND expires_at > ?";

interface PollRow {
  id: string;
  challenge: string;
  status: "pending" | "approved" | "denied" | "consumed";
  expires_at: number;
  desktop_id: string | null;
  handle: string | null;
}

interface PairingRow {
  id: string;
  machine_name: string;
  os: string | null;
  desktop_public_key: string;
  confirmation_code: string;
  status: "pending" | "approved" | "denied" | "consumed";
  expires_at: number;
}

/** Constant-time comparison of two strings of public length. */
function constantTimeEqual(a: string, b: string): boolean {
  if (a.length !== b.length) return false;
  let diff = 0;
  for (let i = 0; i < a.length; i += 1) diff |= a.charCodeAt(i) ^ b.charCodeAt(i);
  return diff === 0;
}

/** `wss://<RP_ORIGIN host>/relay/desktop/<desktopId>` (`ws://` when RP_ORIGIN is http). */
export function relayUrlOf(rpOrigin: string, desktopId: string): string {
  const origin = new URL(rpOrigin);
  return `${origin.protocol === "http:" ? "ws" : "wss"}://${origin.host}/relay/desktop/${desktopId}`;
}

/** `POST /api/pair/start`: validates the keys, caps outstanding requests per IP, stores a pending request. */
export const pairStart: Handler<AppEnv> = async (c) => {
  const body = PairStartBody.safeParse(await c.req.json().catch(() => null));
  if (!body.success) return apiError(c, 400, "invalid", "Invalid pairing request");
  if (decodePublicKey(body.data.publicKey) === null) return apiError(c, 400, "invalid", "publicKey must be 32 bytes, base64url");
  if (decodeBase64Url(body.data.challenge)?.length !== 32) return apiError(c, 400, "invalid", "challenge must be 32 bytes, base64url");
  const limited = await limitOutstandingPairings(c);
  if (limited) return limited;

  const now = nowSeconds();
  const hash = await ipHash(c.env, clientIp(c.req.raw));
  const deviceCode = randomToken(32);
  const deviceCodeHash = await sha256Hex(deviceCode);
  const confirmationCode = newConfirmationCode();
  for (let attempt = 0; attempt < START_ATTEMPTS; attempt += 1) {
    const userCode = newUserCode();
    const result = await c.env.DB.prepare(INSERT_PAIRING_SQL)
      .bind(crypto.randomUUID(), userCode, deviceCodeHash, body.data.challenge, body.data.publicKey, body.data.machineName, body.data.os, confirmationCode, now, now + PAIRING_TTL_SECONDS, hash)
      .run();
    if (result.meta.changes === 1) {
      return respond(c, PairStartResponse, {
        userCode: formatUserCode(userCode),
        deviceCode,
        confirmationCode,
        expiresIn: PAIRING_TTL_SECONDS,
        interval: PAIRING_POLL_INTERVAL_SECONDS,
      }, 201);
    }
  }
  return apiError(c, 409, "conflict", "Could not allocate a pairing code, try again");
};

/**
 * `POST /api/pair/poll`. Unknown device code: 404. A verifier whose SHA-256 is not the stored challenge: 403, no
 * state change, never the token. Then by state: pending (throttled to one write per 2 s), denied, expired or
 * consumed (never the token again), approved (the token is minted here, once).
 */
export const pairPoll: Handler<AppEnv> = async (c) => {
  const body = PairPollBody.safeParse(await c.req.json().catch(() => null));
  if (!body.success) return apiError(c, 400, "invalid", "Invalid poll request");
  const row = await c.env.DB.prepare(POLL_LOOKUP_SQL).bind(await sha256Hex(body.data.deviceCode)).first<PollRow>();
  if (!row) return apiError(c, 404, "not_found", "Unknown device code");

  const verifier = decodeBase64Url(body.data.verifier);
  const proof = verifier === null ? "" : toBase64Url(new Uint8Array(await crypto.subtle.digest("SHA-256", verifier as BufferSource)));
  if (!constantTimeEqual(proof, row.challenge)) return apiError(c, 403, "forbidden", "Verifier does not match");

  const now = nowSeconds();
  switch (row.status) {
    case "consumed":
      return respond(c, PairPollResponse, { status: "expired" });
    case "denied":
      return respond(c, PairPollResponse, { status: "denied" });
    case "pending": {
      if (row.expires_at <= now) return respond(c, PairPollResponse, { status: "expired" });
      const touched = await c.env.DB.prepare(POLL_TOUCH_SQL).bind(now, row.id, now - SLOW_DOWN_SECONDS).run();
      return respond(c, PairPollResponse, { status: touched.meta.changes === 0 ? "slow_down" : "pending" });
    }
    case "approved":
      return mintToken(c, row, now);
  }
};

/** Flips `approved` to `consumed` and inserts the token hash in one batch; the token leaves only with a changed row. */
async function mintToken(c: AppContext, row: PollRow, now: number): Promise<Response> {
  if (row.desktop_id === null || row.handle === null || row.expires_at + PAIRING_COLLECT_GRACE_SECONDS <= now) return respond(c, PairPollResponse, { status: "expired" });
  const token = randomToken(32);
  const [consumed, inserted] = await c.env.DB.batch([
    c.env.DB.prepare(CONSUME_PAIRING_SQL).bind(row.id),
    c.env.DB.prepare(INSERT_TOKEN_SQL).bind(crypto.randomUUID(), await sha256Hex(token), TOKEN_SCOPES, now, row.id, row.desktop_id, now),
  ]);
  if (consumed?.meta.changes !== 1 || inserted?.meta.changes !== 1) return respond(c, PairPollResponse, { status: "expired" });
  return respond(c, PairPollResponse, {
    status: "approved",
    desktopId: row.desktop_id,
    desktopToken: token,
    account: { handle: row.handle },
    relayUrl: relayUrlOf(c.env.RP_ORIGIN, row.desktop_id),
  });
}

/** The request behind a typed user code, only while pending and unexpired; null otherwise. */
async function pendingRequest(c: AppContext, now: number): Promise<{ code: string; row: PairingRow } | null> {
  const code = normalizeUserCode(c.req.param("userCode") ?? "");
  if (code === null) return null;
  const row = await c.env.DB.prepare(PAIRING_BY_CODE_SQL).bind(code).first<PairingRow>();
  return row && row.status === "pending" && row.expires_at > now ? { code, row } : null;
}

const notFound = (c: AppContext): Response => apiError(c, 404, "not_found", "Pairing code not found or expired");

/** `GET /api/pair/:userCode`: what the user compares with the desktop screen. */
export const pairInfo: Handler<AppEnv> = async (c) => {
  requireAuth(c);
  const found = await pendingRequest(c, nowSeconds());
  if (!found) return notFound(c);
  const { row } = found;
  return respond(c, PairInfoResponse, {
    machineName: row.machine_name,
    os: row.os ?? "",
    fingerprint: await fingerprintOfStored(row.desktop_public_key),
    confirmationCode: row.confirmation_code,
    expiresAt: row.expires_at,
  });
};

/** After a batch that approved nothing: why. Quota when the request is still pending, 409 when it was approved meanwhile. */
async function approveFailure(c: AppContext, code: string, now: number, replacing: boolean): Promise<Response> {
  const row = await c.env.DB.prepare(PAIRING_BY_CODE_SQL).bind(code).first<PairingRow>();
  if (row?.status === "pending" && row.expires_at > now) {
    return replacing ? apiError(c, 404, "not_found", "Desktop not found") : apiError(c, 409, "quota_exceeded", "Desktop limit reached");
  }
  if (row && row.status !== "denied" && row.expires_at > now) return apiError(c, 409, "conflict", "Pairing request already approved");
  return notFound(c);
}

/**
 * `POST /api/pair/:userCode/approve` (fresh). Without `replaces`: a new desktop, under the quota, in one batch.
 * With `replaces`: the user's own desktop keeps id, name and color, takes the new key, loses its tokens and every
 * non-revoked grant goes back to `pending`; no quota is used. The room closes every old socket afterwards.
 */
export const pairApprove: Handler<AppEnv> = async (c) => {
  const { user } = requireAuth(c);
  const body = PairApproveBody.safeParse(await c.req.json().catch(() => ({})));
  if (!body.success) return apiError(c, 400, "invalid", "Invalid approve request");
  const now = nowSeconds();
  const found = await pendingRequest(c, now);
  if (!found) {
    const code = normalizeUserCode(c.req.param("userCode") ?? "");
    const row = code === null ? null : await c.env.DB.prepare(PAIRING_BY_CODE_SQL).bind(code).first<PairingRow>();
    return row?.status === "approved" && row.expires_at > now ? apiError(c, 409, "conflict", "Pairing request already approved") : notFound(c);
  }
  const { code, row } = found;
  const fingerprint = await fingerprintOfStored(row.desktop_public_key);

  if (body.data.replaces !== undefined) {
    const desktopId = body.data.replaces;
    const target = await c.env.DB.prepare(REPLACE_TARGET_SQL).bind(desktopId, user.id).first<{ id: string; name: string }>();
    if (!target) return apiError(c, 404, "not_found", "Desktop not found");
    const guard = [row.id, now, desktopId, user.id];
    const results = await c.env.DB.batch([
      c.env.DB.prepare(REPLACE_KEY_SQL).bind(row.id, fingerprint, row.id, desktopId, ...guard),
      c.env.DB.prepare(REPLACE_REVOKE_TOKENS_SQL).bind(now, desktopId, ...guard),
      c.env.DB.prepare(REPLACE_RESET_GRANTS_SQL).bind(now, desktopId, ...guard),
      c.env.DB.prepare(APPROVE_REPLACE_SQL).bind(user.id, desktopId, row.id, now, desktopId, user.id),
      auditStatement(c.env, { action: "pairing.approve", actorUserId: user.id, targetType: "desktop", targetId: desktopId, meta: { desktopId, replaced: true } }, PREVIOUS_CHANGED),
      // After the audit row, which reads `changes()` of the approval above.
      c.env.DB.prepare(DENY_OTHER_APPROVED_SQL).bind(desktopId, row.id, row.id, desktopId),
    ]);
    if (results[3]?.meta.changes !== 1) return approveFailure(c, code, now, true);
    await revokeInRooms(c.env, [desktopId], { all: true }, REVOKED);
    return respond(c, PairApproveResponse, { desktopId, name: target.name });
  }

  const desktopId = crypto.randomUUID();
  const results = await c.env.DB.batch([
    c.env.DB.prepare(INSERT_DESKTOP_SQL).bind(desktopId, user.id, fingerprint, now, row.id, now, user.id, maxDesktopsPerUser(c.env)),
    c.env.DB.prepare(APPROVE_NEW_SQL).bind(user.id, desktopId, row.id, desktopId),
    auditStatement(c.env, { action: "pairing.approve", actorUserId: user.id, targetType: "desktop", targetId: desktopId, meta: { desktopId } }, PREVIOUS_CHANGED),
    // Follows the audit row above: `changes()` is 1 only when that row was written, i.e. the approval happened.
    auditStatement(c.env, { action: "desktop.create", actorUserId: user.id, targetType: "desktop", targetId: desktopId, meta: { desktopId } }, PREVIOUS_CHANGED),
  ]);
  if (results[1]?.meta.changes !== 1) return approveFailure(c, code, now, false);
  return respond(c, PairApproveResponse, { desktopId, name: row.machine_name });
};

/** `POST /api/pair/:userCode/deny`: pending to denied; anything else is 404. */
export const pairDeny: Handler<AppEnv> = async (c) => {
  const { user } = requireAuth(c);
  const code = normalizeUserCode(c.req.param("userCode") ?? "");
  if (code === null) return notFound(c);
  const result = await c.env.DB.prepare(DENY_PAIRING_SQL).bind(user.id, code, nowSeconds()).run();
  if (result.meta.changes === 0) return notFound(c);
  return c.json({ ok: true as const });
};
