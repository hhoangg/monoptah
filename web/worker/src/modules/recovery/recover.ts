// `POST /api/auth/recover` (A-05): sign in with a recovery code. The answer and the work done are the same for an
// unknown handle, a wrong code, a used code and a blocked user: the HMAC is always computed and ONE lookup query
// (users joined to recovery_codes by handle and code hash) always runs; any miss is `401 unauthorized`. On a hit,
// ONE batch makes the `recovery_only` session, marks the code used, deletes every other session of the user and
// audits. Desktops, devices, grants and invites are never touched.
import type { Handler } from "hono";
import { RecoverBody, RecoverResponse } from "@monoptah/protocol";
import { apiError, respond, type AppContext, type AppEnv } from "../../http.ts";
import { clientIp, ipHash } from "../../lib/ip.ts";
import { nowSeconds } from "../../lib/time.ts";
import { auditStatement } from "../audit/audit.ts";
import { limitHandleAttempts } from "../ratelimit/index.ts";
import { closeSessionSockets, createSession, listSessionIdHashes, SESSION_CREATED_SQL } from "../sessions/session.ts";
import { hashRecoveryCode } from "./codes.ts";

/**
 * The single lookup of the sign-in: the user with this handle that owns an unused code with this hash.
 * `unused` counts the user's unused codes (this one included). Uses `users_handle_unique`, then
 * `recovery_codes_user_id_idx`.
 */
export const RECOVER_LOOKUP_SQL = `SELECT u.id, u.handle, u.display_name, u.role, rc.id AS code_id,
  (SELECT count(*) FROM recovery_codes WHERE user_id = u.id AND used_at IS NULL) AS unused
  FROM users u JOIN recovery_codes rc ON rc.user_id = u.id AND rc.code_hash = ? AND rc.used_at IS NULL
  WHERE u.handle = ? AND u.status = 'active'`;
export const MARK_CODE_USED_SQL = "UPDATE recovery_codes SET used_at = ? WHERE id = ? AND used_at IS NULL";
export { SESSION_CREATED_SQL };
export const DELETE_OTHER_SESSIONS_SQL = `DELETE FROM sessions WHERE user_id = ? AND id_hash <> ? AND ${SESSION_CREATED_SQL}`;
export const CODE_UNUSED_SQL = "EXISTS (SELECT 1 FROM recovery_codes WHERE id = ? AND user_id = ? AND used_at IS NULL)";

interface LookupRow {
  id: string;
  handle: string;
  display_name: string;
  role: "user" | "admin";
  code_id: string;
  unused: number;
}

/** Longest recovery code accepted (a code is 19 characters with its hyphens, or 16 without). */
export const MAX_CODE_LENGTH = 64;

/** The one answer for every failure. */
const failed = (c: AppContext): Response => apiError(c, 401, "unauthorized", "Recovery failed");

export const recover: Handler<AppEnv> = async (c) => {
  const body = RecoverBody.safeParse(await c.req.json().catch(() => null));
  if (!body.success) return apiError(c, 400, "invalid", "Invalid recovery request");
  const { handle, code } = body.data;
  // A real code is 16 characters. Anything longer is refused before it is hashed (an unbounded string would make
  // the HMAC cost the attacker's choice) and without spending the handle's attempts: the same uniform 401.
  if (code.length > MAX_CODE_LENGTH) return failed(c);

  const limited = await limitHandleAttempts(c, handle);
  if (limited) return limited;

  const codeHash = await hashRecoveryCode(c.env, code);
  const row = await c.env.DB.prepare(RECOVER_LOOKUP_SQL).bind(codeHash, handle).first<LookupRow>();
  if (!row) return failed(c);

  const now = nowSeconds();
  const codesLeft = Math.max(0, row.unused - 1);
  const oldSessions = await listSessionIdHashes(c.env, row.id);
  const session = await createSession(c.env, {
    userId: row.id,
    amr: "recovery",
    recoveryOnly: true,
    userAgent: c.req.header("User-Agent"),
    now,
    when: { sql: CODE_UNUSED_SQL, params: [row.code_id, row.id] },
  });
  const created = { sql: SESSION_CREATED_SQL, params: [session.idHash] };
  const ip = await ipHash(c.env, clientIp(c.req.raw));
  const results = await c.env.DB.batch([
    session.statement,
    c.env.DB.prepare(MARK_CODE_USED_SQL).bind(now, row.code_id),
    c.env.DB.prepare(DELETE_OTHER_SESSIONS_SQL).bind(row.id, session.idHash, ...created.params),
    auditStatement(
      c.env,
      { action: "recovery.use", meta: { codesLeft, revokedSessions: oldSessions.length, ipHash: ip }, actorUserId: row.id, targetType: "user", targetId: row.id, ts: now },
      created,
    ),
  ]);
  // A parallel use of the same code won the race: nothing was written here.
  if ((results[0]?.meta.changes ?? 0) === 0) return failed(c);

  await closeSessionSockets(c.env, row.id, oldSessions);
  c.header("Set-Cookie", session.cookie, { append: true });
  return respond(c, RecoverResponse, {
    user: { id: row.id, handle: row.handle, displayName: row.display_name, role: row.role },
    recoveryOnly: true,
    codesLeft,
  });
};
