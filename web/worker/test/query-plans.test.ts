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
import { CONSUME_CHALLENGE_SQL, INSERT_CHALLENGE_SQL } from "../src/modules/passkeys/challenges.ts";
import { INVITES_USED_SQL, ME_ROW_SQL, PATCH_DISPLAY_NAME_SQL } from "../src/modules/me/handlers.ts";
import { CONSUME_SQL } from "../src/modules/ratelimit/limiter.ts";
import { LIST_USER_SESSIONS_SQL } from "../src/modules/sessions/handlers.ts";
import {
  DELETE_ALL_SESSIONS_EXCEPT_SQL,
  DELETE_ALL_SESSIONS_SQL,
  DELETE_SESSION_SQL,
  LIST_DESKTOP_IDS_SQL,
  LIST_SESSION_HASHES_EXCEPT_SQL,
  LIST_SESSION_HASHES_SQL,
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
   * An INSERT into `users` also lists the foreign key checks SQLite compiles for the child tables of `users`.
   * They run only when a `users` row is deleted (account deletion, A-09), not on this INSERT, and four of those
   * child columns have no index (see the report of A-03). Only these exact tables may show a SCAN, only here.
   */
  fkChildScans?: string[];
}

const CASES: PlanCase[] = [
  { name: "rate limit consume", sql: CONSUME_SQL, bind: ["k", 0] },
  { name: "session lookup (join users)", sql: SESSION_LOOKUP_SQL, bind: ["h"] },
  { name: "session touch", sql: SESSION_TOUCH_SQL, bind: [1, 2, "h", 0] },
  { name: "list own sessions", sql: LIST_USER_SESSIONS_SQL, bind: ["u", 1, 1] },
  { name: "delete one session", sql: DELETE_SESSION_SQL, bind: ["u", "h"] },
  { name: "delete all sessions", sql: DELETE_ALL_SESSIONS_SQL, bind: ["u"] },
  { name: "delete all sessions except one", sql: DELETE_ALL_SESSIONS_EXCEPT_SQL, bind: ["u", "h"] },
  { name: "list session hashes", sql: LIST_SESSION_HASHES_SQL, bind: ["u"] },
  { name: "list session hashes except one", sql: LIST_SESSION_HASHES_EXCEPT_SQL, bind: ["u", "h"] },
  { name: "list desktops of a user", sql: LIST_DESKTOP_IDS_SQL, bind: ["u", 5] },
  { name: "me row", sql: ME_ROW_SQL, bind: ["u"] },
  { name: "invites used by a user", sql: INVITES_USED_SQL, bind: ["u"] },
  { name: "patch display name", sql: PATCH_DISPLAY_NAME_SQL, bind: ["n", "u"] },
  { name: "invite lookup by code hash", sql: INVITE_LOOKUP_SQL, bind: ["h", 1] },
  { name: "handle taken", sql: HANDLE_TAKEN_SQL, bind: ["h"] },
  { name: "invites_paused setting", sql: INVITES_PAUSED_SQL, bind: [] },
  { name: "user count (cap)", sql: USER_COUNT_SQL, bind: [], usersCountScan: true },
  { name: "registration: insert user", sql: INSERT_USER_SQL, bind: ["u", "h", "n", 1, "i", "h", 1, 0, 0], usersCountScan: true, fkChildScans: ["settings", "pairing_requests", "invites", "challenges"] },
  { name: "registration: use invite", sql: USE_INVITE_SQL, bind: ["u", 1, "i", "u", "i"] },
  { name: "registration: insert passkey", sql: INSERT_PASSKEY_SQL, bind: ["c", "u", "k", 0, "[]", "d", 0, "n", 1, "u", "i"] },
  { name: "registration: insert recovery code", sql: INSERT_RECOVERY_CODE_SQL, bind: ["i", "u", "h", 1, "u", "i"] },
  { name: "role of a user", sql: USER_ROLE_SQL, bind: ["u"] },
  { name: "insert challenge", sql: INSERT_CHALLENGE_SQL, bind: ["i", "login", null, "c", null, 1] },
  { name: "consume challenge", sql: CONSUME_CHALLENGE_SQL, bind: ["i", "login", null, 1] },
  { name: "passkey by credential id (join users)", sql: PASSKEY_LOGIN_SQL, bind: ["c"] },
  { name: "passkey use update", sql: UPDATE_PASSKEY_USE_SQL, bind: [1, 0, 1, "c"] },
  { name: "passkeys of a user", sql: USER_PASSKEYS_SQL, bind: ["u"] },
  { name: "set reauth_at", sql: SET_REAUTH_SQL, bind: [1, "h"] },
  { name: "audit first page", sql: AUDIT_PAGE_SQL, bind: ["u", 51], noTempSort: true },
  { name: "audit next page", sql: AUDIT_PAGE_AFTER_SQL, bind: ["u", 1, "id", 51], noTempSort: true },
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
      if (c.fkChildScans?.some((table) => detail === `SCAN ${table}`)) continue;
      // `INSERT ... SELECT ... WHERE <condition>` reads one constant row, not a table.
      if (detail === "SCAN CONSTANT ROW") continue;
      expect(detail, detail).not.toMatch(/^SCAN /);
      if (c.noTempSort) expect(detail, detail).not.toMatch(/TEMP B-TREE/);
    }
    if (!isInsert && !c.usersCountScan) expect(details.some((d) => /^SEARCH .* USING (COVERING )?INDEX/.test(d)), details.join(" | ")).toBe(true);
  });

  it("the audit paging queries use the (actor_user_id, ts, id) index", async () => {
    for (const [sql, bind] of [[AUDIT_PAGE_SQL, ["u", 51]], [AUDIT_PAGE_AFTER_SQL, ["u", 1, "id", 51]]] as const) {
      const { results } = await env.DB.prepare(`EXPLAIN QUERY PLAN ${sql}`).bind(...bind).all<{ detail: string }>();
      expect(results.map((r) => r.detail).join(" | ")).toContain("audit_log_actor_user_id_ts_id_idx");
    }
  });
});
