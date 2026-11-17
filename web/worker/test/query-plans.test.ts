// EXPLAIN QUERY PLAN of every query the accounts modules run per request: none may scan a whole table.
// Add each new query here (with the real SQL constant exported by its module), never weaken the check.
import { env } from "cloudflare:workers";
import { describe, expect, it } from "vitest";
import { SESSION_LOOKUP_SQL, SESSION_TOUCH_SQL } from "../src/middleware/auth.ts";
import { AUDIT_PAGE_AFTER_SQL, AUDIT_PAGE_SQL } from "../src/modules/audit/handlers.ts";
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
  { name: "audit first page", sql: AUDIT_PAGE_SQL, bind: ["u", 51], noTempSort: true },
  { name: "audit next page", sql: AUDIT_PAGE_AFTER_SQL, bind: ["u", 1, "id", 51], noTempSort: true },
];

describe("query plans", () => {
  it.each(CASES.map((c) => [c.name, c] as const))("%s: no full table scan", async (_name, c) => {
    const { results } = await env.DB.prepare(`EXPLAIN QUERY PLAN ${c.sql}`).bind(...c.bind).all<{ detail: string }>();
    const details = results.map((r) => r.detail);
    const isInsert = /^\s*INSERT/i.test(c.sql);
    // An INSERT ... ON CONFLICT has no plan rows: its conflict target is the primary key (checked in schema.test.ts).
    if (!isInsert) expect(details.length).toBeGreaterThan(0);
    for (const detail of details) {
      expect(detail, detail).not.toMatch(/^SCAN /);
      if (c.noTempSort) expect(detail, detail).not.toMatch(/TEMP B-TREE/);
    }
    if (!isInsert) expect(details.some((d) => /^SEARCH .* USING (COVERING )?INDEX/.test(d)), details.join(" | ")).toBe(true);
  });

  it("the audit paging queries use the (actor_user_id, ts, id) index", async () => {
    for (const [sql, bind] of [[AUDIT_PAGE_SQL, ["u", 51]], [AUDIT_PAGE_AFTER_SQL, ["u", 1, "id", 51]]] as const) {
      const { results } = await env.DB.prepare(`EXPLAIN QUERY PLAN ${sql}`).bind(...bind).all<{ detail: string }>();
      expect(results.map((r) => r.detail).join(" | ")).toContain("audit_log_actor_user_id_ts_id_idx");
    }
  });
});
