// EXPLAIN QUERY PLAN of every query the accounts modules run per request: none may scan a whole table.
// Add each new query here (with the real SQL constant exported by its module), never weaken the check.
import { env } from "cloudflare:workers";
import { describe, expect, it } from "vitest";
import { SESSION_LOOKUP_SQL, SESSION_TOUCH_SQL } from "../src/middleware/auth.ts";
import { AUDIT_PAGE_AFTER_SQL, AUDIT_PAGE_SQL } from "../src/modules/audit/handlers.ts";
import { PASSKEY_LOGIN_SQL, UPDATE_PASSKEY_USE_SQL } from "../src/modules/auth/login.ts";
import { SET_REAUTH_SQL, USER_PASSKEYS_SQL } from "../src/modules/auth/reauth.ts";
import {
  HANDLE_TAKEN_SQL,
  INSERT_PASSKEY_SQL,
  INSERT_RECOVERY_CODE_SQL,
  INSERT_USER_SQL,
  INVITE_LOOKUP_SQL,
  USE_INVITE_SQL,
  USER_ROLE_SQL,
} from "../src/modules/auth/register.ts";
import { INVITES_PAUSED_SQL, USER_COUNT_SQL } from "../src/modules/invites/gate.ts";
import {
  ADD_PASSKEY_EXCLUDE_SQL,
  COUNT_PASSKEYS_SQL,
  DELETE_PASSKEY_CONDITION_SQL,
  DELETE_PASSKEY_SQL,
  GET_PASSKEY_SQL,
  INSERT_ADDED_PASSKEY_SQL,
  LIST_PASSKEYS_SQL,
  PASSKEY_EXISTS_SQL,
  RENAME_PASSKEY_SQL,
} from "../src/modules/passkeys/manage.ts";
import { CODE_UNUSED_SQL, DELETE_OTHER_SESSIONS_SQL, MARK_CODE_USED_SQL, RECOVER_LOOKUP_SQL, SESSION_CREATED_SQL } from "../src/modules/recovery/recover.ts";
import {
  CLEAR_ACK_SQL,
  DELETE_OLD_CODES_SQL,
  INSERT_FIRST_CODE_SQL,
  INSERT_NEXT_CODE_SQL,
  MAX_BATCH_SQL,
  ACKED_BATCH_SQL,
  SET_ACK_SQL,
} from "../src/modules/recovery/regenerate.ts";
import {
  ADMIN_AUDIT_PAGE_AFTER_SQL,
  ADMIN_AUDIT_PAGE_SQL,
  GRANT_INVITES_SQL,
  LIST_USERS_SQL,
  UNBLOCK_SQL,
  UPSERT_SETTING_SQL,
  USER_STATUS_SQL,
} from "../src/modules/admin/handlers.ts";
import {
  blockUsersSql,
  deletePushSubscriptionsSql,
  deleteSessionsSql,
  listDesktopsSql,
  revokeDesktopGrantsSql,
  revokeDeviceGrantsSql,
  revokeInvitesSql,
  revokeTokensSql,
} from "../src/modules/admin/branch.ts";
import { INSERT_INVITE_SQL, INVITE_BY_ID_SQL, LIST_INVITES_SQL, REVOKE_INVITE_SQL } from "../src/modules/invites/handlers.ts";
import { CONSUME_CHALLENGE_SQL, INSERT_CHALLENGE_SQL } from "../src/modules/passkeys/challenges.ts";
import { INVITES_USED_SQL, ME_ROW_SQL, PATCH_DISPLAY_NAME_SQL, RECOVERY_LEFT_SQL } from "../src/modules/me/handlers.ts";
import { CONSUME_SQL } from "../src/modules/ratelimit/limiter.ts";
import { OUTSTANDING_PAIRINGS_SQL } from "../src/modules/ratelimit/pairings.ts";
import { LIST_USER_SESSIONS_SQL, LOGOUT_PUSH_SQL } from "../src/modules/sessions/handlers.ts";
import {
  DELETE_ALL_SESSIONS_EXCEPT_SQL,
  DELETE_ALL_SESSIONS_SQL,
  DELETE_SESSION_SQL,
  LIST_DESKTOP_IDS_SQL,
  LIST_SESSION_HASHES_EXCEPT_SQL,
  LIST_SESSION_HASHES_SQL,
  SESSION_BY_HASH_SQL,
} from "../src/modules/sessions/session.ts";

interface PlanCase {
  name: string;
  sql: string;
  bind: unknown[];
  /** The query must read rows in index order, without a temporary sort (paged queries). */
  noTempSort?: boolean;
  /**
   * The query counts the `users` table (the MAX_USERS cap), which is bounded by that cap, so it may scan a
   * covering index of `users` (never the table itself) and nothing else.
   */
  usersCountScan?: boolean;
  /**
   * The query lists every row of `users` (`GET /api/admin/users`, bounded by the user cap). It may walk the table in
   * the order of `users_created_at_idx` (no temporary sort) and nothing else may scan.
   */
  usersListScan?: boolean;
  /**
   * The query walks the invite tree with the recursive CTE of `branch.ts`. SQLite shows the queue of the recursion
   * (`SCAN b`) and the materialized, small result (`SCAN branch`) as scans; the step itself must search
   * `users_invited_by_idx`, which is checked. Nothing else may scan.
   */
  branchCte?: boolean;
  /**
   * The first page of a paged query that has no `WHERE`: it reads `LIMIT` rows in the order of this index and stops
   * (the SQL must contain `LIMIT`). The next pages are real searches and take no allowance.
   */
  limitedIndexScan?: { table: string; index: string };
}

const CASES: PlanCase[] = [
  { name: "rate limit consume", sql: CONSUME_SQL, bind: ["k", 0, 1] },
  { name: "outstanding pairing requests of an IP", sql: OUTSTANDING_PAIRINGS_SQL, bind: ["h", 1] },
  { name: "session lookup (join users)", sql: SESSION_LOOKUP_SQL, bind: ["h"] },
  { name: "session touch", sql: SESSION_TOUCH_SQL, bind: [1, 2, "h", 0] },
  { name: "list own sessions", sql: LIST_USER_SESSIONS_SQL, bind: ["u", 1, 1] },
  { name: "delete one session", sql: DELETE_SESSION_SQL, bind: ["u", "h"] },
  { name: "delete all sessions", sql: DELETE_ALL_SESSIONS_SQL, bind: ["u"] },
  { name: "delete all sessions except one", sql: DELETE_ALL_SESSIONS_EXCEPT_SQL, bind: ["u", "h"] },
  { name: "session row of the request cookie", sql: SESSION_BY_HASH_SQL, bind: ["h"] },
  { name: "list session hashes", sql: LIST_SESSION_HASHES_SQL, bind: ["u"] },
  { name: "list session hashes except one", sql: LIST_SESSION_HASHES_EXCEPT_SQL, bind: ["u", "h"] },
  { name: "list desktops of a user", sql: LIST_DESKTOP_IDS_SQL, bind: ["u"] },
  { name: "me row", sql: ME_ROW_SQL, bind: ["u"] },
  { name: "invites used by a user", sql: INVITES_USED_SQL, bind: ["u"] },
  { name: "unused recovery codes of a user", sql: RECOVERY_LEFT_SQL, bind: ["u"] },
  { name: "logout: delete the push subscriptions of a device", sql: LOGOUT_PUSH_SQL, bind: ["d", "u"] },
  { name: "patch display name", sql: PATCH_DISPLAY_NAME_SQL, bind: ["n", "u"] },
  { name: "invite lookup by code hash", sql: INVITE_LOOKUP_SQL, bind: ["h", 1] },
  { name: "handle taken", sql: HANDLE_TAKEN_SQL, bind: ["h"] },
  { name: "invites_paused setting", sql: INVITES_PAUSED_SQL, bind: [] },
  { name: "user count (cap)", sql: USER_COUNT_SQL, bind: [], usersCountScan: true },
  { name: "registration: insert user", sql: INSERT_USER_SQL, bind: ["u", "h", "n", 1, "i", "h", 1, 0, 0], usersCountScan: true },
  { name: "registration: use invite", sql: USE_INVITE_SQL, bind: ["u", 1, "i", "u", "i"] },
  { name: "registration: insert passkey", sql: INSERT_PASSKEY_SQL, bind: ["c", "u", "k", 0, "[]", "d", 0, "n", 1, "u", "i"] },
  { name: "registration: insert recovery code", sql: INSERT_RECOVERY_CODE_SQL, bind: ["i", "u", "h", 1, "u", "i"] },
  { name: "role of a user", sql: USER_ROLE_SQL, bind: ["u"] },
  { name: "insert challenge", sql: INSERT_CHALLENGE_SQL, bind: ["i", "login", null, "c", null, 1] },
  { name: "consume challenge", sql: CONSUME_CHALLENGE_SQL, bind: ["i", "login", null, 1] },
  { name: "passkey by credential id (join users)", sql: PASSKEY_LOGIN_SQL, bind: ["c"] },
  { name: "passkey use update", sql: UPDATE_PASSKEY_USE_SQL, bind: [1, 0, 1, "c", 1, 1] },
  { name: "passkeys of a user", sql: USER_PASSKEYS_SQL, bind: ["u"] },
  { name: "set reauth_at", sql: SET_REAUTH_SQL, bind: [1, "h"] },
  { name: "list own invites", sql: LIST_INVITES_SQL, bind: ["u"] },
  { name: "create invite (quota and gate)", sql: INSERT_INVITE_SQL, bind: ["i", "h", 1, 1, "u", 0, 0], usersCountScan: true },
  { name: "invite by id", sql: INVITE_BY_ID_SQL, bind: ["i"] },
  { name: "revoke invite", sql: REVOKE_INVITE_SQL, bind: [1, "i", "u"] },
  { name: "admin: list users", sql: LIST_USERS_SQL, bind: [], usersListScan: true },
  { name: "admin: user status", sql: USER_STATUS_SQL, bind: ["u"] },
  { name: "admin: unblock", sql: UNBLOCK_SQL, bind: ["u"] },
  { name: "admin: grant invites", sql: GRANT_INVITES_SQL, bind: [1, "u", 1, 1000] },
  { name: "admin: upsert setting", sql: UPSERT_SETTING_SQL, bind: ["1", 1, "u"] },
  { name: "admin: audit first page", sql: ADMIN_AUDIT_PAGE_SQL, bind: [51], noTempSort: true, limitedIndexScan: { table: "audit_log", index: "audit_log_ts_id_idx" } },
  { name: "admin: audit next page", sql: ADMIN_AUDIT_PAGE_AFTER_SQL, bind: [1, "id", 51], noTempSort: true },
  { name: "block (cascade): list desktops", sql: listDesktopsSql(true), bind: ["r", "a"], branchCte: true },
  { name: "block (cascade): mark users blocked", sql: blockUsersSql(true), bind: ["r", 1, "x", "r", "a"], branchCte: true },
  { name: "block (cascade): delete sessions", sql: deleteSessionsSql(true), bind: ["r", "a"], branchCte: true },
  { name: "block (cascade): revoke tokens", sql: revokeTokensSql(true), bind: ["r", 1, "a"], branchCte: true },
  { name: "block (cascade): revoke grants of desktops", sql: revokeDesktopGrantsSql(true), bind: ["r", 1, "a"], branchCte: true },
  { name: "block (cascade): revoke grants of devices", sql: revokeDeviceGrantsSql(true), bind: ["r", 1, "a"], branchCte: true },
  { name: "block (cascade): delete push subscriptions", sql: deletePushSubscriptionsSql(true), bind: ["r", "a"], branchCte: true },
  { name: "block (cascade): revoke invites", sql: revokeInvitesSql(true), bind: ["r", 1, "a"], branchCte: true },
  { name: "block (single): list desktops", sql: listDesktopsSql(false), bind: ["r", "a"] },
  { name: "block (single): mark users blocked", sql: blockUsersSql(false), bind: ["r", 1, "x", "r", "a"] },
  { name: "block (single): delete sessions", sql: deleteSessionsSql(false), bind: ["r", "a"] },
  { name: "block (single): revoke tokens", sql: revokeTokensSql(false), bind: ["r", 1, "a"] },
  { name: "block (single): revoke grants of desktops", sql: revokeDesktopGrantsSql(false), bind: ["r", 1, "a"] },
  { name: "block (single): revoke grants of devices", sql: revokeDeviceGrantsSql(false), bind: ["r", 1, "a"] },
  { name: "block (single): delete push subscriptions", sql: deletePushSubscriptionsSql(false), bind: ["r", "a"] },
  { name: "block (single): revoke invites", sql: revokeInvitesSql(false), bind: ["r", 1, "a"] },
  { name: "audit first page", sql: AUDIT_PAGE_SQL, bind: ["u", 51], noTempSort: true },
  { name: "audit next page", sql: AUDIT_PAGE_AFTER_SQL, bind: ["u", 1, "id", 51], noTempSort: true },
  { name: "recover: lookup by handle and code hash", sql: RECOVER_LOOKUP_SQL, bind: ["h", "handle"] },
  { name: "recover: mark code used", sql: MARK_CODE_USED_SQL, bind: [1, "c"] },
  { name: "recover: delete other sessions", sql: DELETE_OTHER_SESSIONS_SQL, bind: ["u", "h", "h"] },
  { name: "recover: session created condition", sql: `SELECT 1 WHERE ${SESSION_CREATED_SQL}`, bind: ["h"] },
  { name: "recover: code unused condition", sql: `SELECT 1 WHERE ${CODE_UNUSED_SQL}`, bind: ["c", "u"] },
  { name: "regenerate: max batch", sql: MAX_BATCH_SQL, bind: ["u"] },
  { name: "regenerate: first code", sql: INSERT_FIRST_CODE_SQL, bind: ["i", "u", "h", 2, 1, "u", 1] },
  { name: "regenerate: next code", sql: INSERT_NEXT_CODE_SQL, bind: ["i", "u", "h", 2, 1, "i"] },
  { name: "regenerate: delete old codes", sql: DELETE_OLD_CODES_SQL, bind: ["u", 1, "i"] },
  { name: "regenerate: clear ack", sql: CLEAR_ACK_SQL, bind: ["u", "i"] },
  { name: "recovery ack", sql: SET_ACK_SQL, bind: [1, "u", "u", 1] },
  { name: "recovery ack, repeated", sql: ACKED_BATCH_SQL, bind: ["u", "u", 1] },
  { name: "passkeys: list own", sql: LIST_PASSKEYS_SQL, bind: ["u"] },
  { name: "passkeys: get own by id", sql: GET_PASSKEY_SQL, bind: ["c", "u"] },
  { name: "passkeys: exclude list", sql: ADD_PASSKEY_EXCLUDE_SQL, bind: ["u"] },
  { name: "passkeys: insert added", sql: INSERT_ADDED_PASSKEY_SQL, bind: ["c", "u", "k", 0, "[]", "d", 0, null, "u", 1] },
  { name: "passkeys: rename", sql: RENAME_PASSKEY_SQL, bind: ["n", "c", "u"] },
  { name: "passkeys: exists condition", sql: `SELECT 1 WHERE ${PASSKEY_EXISTS_SQL}`, bind: ["c", "u"] },
  { name: "passkeys: count", sql: COUNT_PASSKEYS_SQL, bind: ["u"] },
  { name: "passkeys: delete condition", sql: `SELECT 1 WHERE ${DELETE_PASSKEY_CONDITION_SQL}`, bind: ["c", "u", "u"] },
  { name: "passkeys: delete", sql: DELETE_PASSKEY_SQL, bind: ["c", "u", "u"] },
];

describe("query plans", () => {
  it.each(CASES.map((c) => [c.name, c] as const))("%s: no full table scan", async (_name, c) => {
    const { results } = await env.DB.prepare(`EXPLAIN QUERY PLAN ${c.sql}`).bind(...c.bind).all<{ detail: string }>();
    const details = results.map((r) => r.detail);
    const isInsert = /^\s*INSERT/i.test(c.sql);
    // An INSERT ... ON CONFLICT has no plan rows: its conflict target is the primary key (checked in schema.test.ts).
    if (!isInsert) expect(details.length).toBeGreaterThan(0);
    for (const detail of details) {
      if (c.usersCountScan && /^SCAN users USING COVERING INDEX /.test(detail)) continue;
      // `INSERT ... SELECT ... WHERE <condition>` reads one constant row, not a table.
      if (detail === "SCAN CONSTANT ROW") continue;
      if (c.branchCte && (detail === "SCAN b" || detail === "SCAN branch")) continue;
      if (c.limitedIndexScan && /\bLIMIT\b/.test(c.sql) && detail === `SCAN ${c.limitedIndexScan.table} USING INDEX ${c.limitedIndexScan.index}`) continue;
      if (c.usersListScan && /^SCAN u USING INDEX users_created_at_idx$/.test(detail)) continue;
      expect(detail, detail).not.toMatch(/^SCAN /);
      if (c.noTempSort) expect(detail, detail).not.toMatch(/TEMP B-TREE/);
    }
    if (c.branchCte) expect(details, details.join(" | ")).toContain("SEARCH u USING INDEX users_invited_by_idx (invited_by=?)");
    if (!isInsert && !c.usersCountScan && !c.usersListScan && !c.limitedIndexScan) expect(details.some((d) => /^SEARCH .* USING (COVERING )?INDEX/.test(d)), details.join(" | ")).toBe(true);
  });

  it("the outstanding pairing count is answered by the (ip_hash, status, expires_at) index", async () => {
    const { results } = await env.DB.prepare(`EXPLAIN QUERY PLAN ${OUTSTANDING_PAIRINGS_SQL}`).bind("h", 1).all<{ detail: string }>();
    expect(results.map((r) => r.detail).join(" | ")).toContain("SEARCH pairing_requests USING COVERING INDEX pairing_requests_ip_status_expires_idx (ip_hash=? AND status=? AND expires_at>?)");
  });

  it("the audit paging queries use the (actor_user_id, ts, id) index", async () => {
    for (const [sql, bind] of [[AUDIT_PAGE_SQL, ["u", 51]], [AUDIT_PAGE_AFTER_SQL, ["u", 1, "id", 51]]] as const) {
      const { results } = await env.DB.prepare(`EXPLAIN QUERY PLAN ${sql}`).bind(...bind).all<{ detail: string }>();
      expect(results.map((r) => r.detail).join(" | ")).toContain("audit_log_actor_user_id_ts_id_idx");
    }
  });
});
