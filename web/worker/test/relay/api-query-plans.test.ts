// EXPLAIN QUERY PLAN of every SQL of the pairing, desktops, devices and grants modules: none may scan a table
// (pattern of test/query-plans.test.ts). The SQL comes from the exported constants, never a copy.
import { env } from "cloudflare:workers";
import { describe, expect, it } from "vitest";
import {
  DELETE_DENY_APPROVED_SQL,
  DESKTOP_ME_SQL,
  GET_AVATAR_SQL,
  GET_DESKTOP_SQL,
  LIST_DESKTOPS_SQL,
  PATCH_DESKTOP_SQL,
  PENDING_COUNT_ONE_SQL,
  PENDING_COUNTS_SQL,
  REVOKE_DESKTOP_GRANTS_SQL,
  REVOKE_DESKTOP_SQL,
  REVOKE_DESKTOP_TOKENS_SQL,
  UNPAIR_DENY_APPROVED_SQL,
  UNPAIR_TOKENS_SQL,
} from "../../src/modules/desktops/handlers.ts";
import { TOKEN_LOOKUP_SQL, TOKEN_TOUCH_SQL } from "../../src/modules/desktops/token.ts";
import {
  DEVICE_GRANTS_SQL,
  FIND_DEVICE_SQL,
  FORGET_DEVICE_SQL,
  FORGET_GRANTS_SQL,
  FORGET_PUSH_SQL,
  INSERT_DEVICE_SQL,
  LIST_DEVICES_SQL,
  PATCH_DEVICE_SQL,
} from "../../src/modules/devices/handlers.ts";
import { CLEANUP_QUERIES as GRANT_CLEANUP } from "../../src/modules/grants/cleanup.ts";
import {
  CONFIRM_GRANT_SQL,
  CONSUME_SELECTION_TICKET_SQL,
  CONSUME_TERMINAL_TICKET_SQL,
  INSERT_TERMINAL_TICKET_SQL,
  CONTROL_GRANT_SQL,
  INSERT_SELECTION_TICKET_SQL,
  DESKTOP_GRANT_STATE_SQL,
  DESKTOP_GRANTS_SQL,
  GET_GRANT_SQL,
  INSERT_GRANT_SQL,
  LIST_GRANTS_SQL,
  OWN_DESKTOP_SQL,
  OWN_DEVICE_SQL,
  PENDING_GRANT_SQL,
  REQUEST_PERMISSION_SQL,
  REVOKE_GRANT_SQL,
} from "../../src/modules/grants/handlers.ts";
import { CLEANUP_QUERIES as PAIRING_CLEANUP } from "../../src/modules/pairing/cleanup.ts";
import {
  APPROVE_NEW_SQL,
  APPROVE_REPLACE_SQL,
  CONSUME_PAIRING_SQL,
  DENY_OTHER_APPROVED_SQL,
  DENY_PAIRING_SQL,
  INSERT_DESKTOP_SQL,
  INSERT_PAIRING_SQL,
  INSERT_TOKEN_SQL,
  PAIRING_BY_CODE_SQL,
  POLL_LOOKUP_SQL,
  POLL_TOUCH_SQL,
  REPLACE_KEY_SQL,
  REPLACE_RESET_GRANTS_SQL,
  REPLACE_REVOKE_TOKENS_SQL,
  REPLACE_TARGET_SQL,
} from "../../src/modules/pairing/handlers.ts";

interface PlanCase {
  name: string;
  sql: string;
  bind: unknown[];
}

const s = "x";
const CASES: PlanCase[] = [
  { name: "pair/start: insert request", sql: INSERT_PAIRING_SQL, bind: [s, s, s, s, s, s, s, s, s, 1, 2, s] },
  { name: "pair/poll: lookup by device code hash", sql: POLL_LOOKUP_SQL, bind: [s] },
  { name: "pair/poll: touch", sql: POLL_TOUCH_SQL, bind: [1, s, 0] },
  { name: "pair/poll: consume", sql: CONSUME_PAIRING_SQL, bind: [s] },
  { name: "pair/poll: insert token", sql: INSERT_TOKEN_SQL, bind: [s, s, s, 1, s, s, 1] },
  { name: "pair: request by user code", sql: PAIRING_BY_CODE_SQL, bind: [s] },
  { name: "approve: insert desktop (quota)", sql: INSERT_DESKTOP_SQL, bind: [s, s, s, 1, s, 1, s, 5] },
  { name: "approve: bind request", sql: APPROVE_NEW_SQL, bind: [s, s, s, s] },
  { name: "replace: target desktop", sql: REPLACE_TARGET_SQL, bind: [s, s] },
  { name: "replace: swap key", sql: REPLACE_KEY_SQL, bind: [s, s, s, s, s, s, 1, s, s] },
  { name: "replace: revoke tokens", sql: REPLACE_REVOKE_TOKENS_SQL, bind: [1, s, s, 1, s, s] },
  { name: "replace: reset grants", sql: REPLACE_RESET_GRANTS_SQL, bind: [1, s, s, 1, s, s] },
  { name: "replace: bind request", sql: APPROVE_REPLACE_SQL, bind: [s, s, s, 1, s, s] },
  { name: "replace: deny other approved requests", sql: DENY_OTHER_APPROVED_SQL, bind: [s, s, s, s] },
  { name: "unpair: deny approved requests", sql: UNPAIR_DENY_APPROVED_SQL, bind: [s] },
  { name: "delete desktop: deny approved requests", sql: DELETE_DENY_APPROVED_SQL, bind: [s, s, s, 1] },
  { name: "deny", sql: DENY_PAIRING_SQL, bind: [s, s, 1] },
  { name: "cleanup: pairing requests", sql: (PAIRING_CLEANUP[0] as { sql: string }).sql, bind: [1] },
  { name: "token lookup", sql: TOKEN_LOOKUP_SQL, bind: [s] },
  { name: "token touch", sql: TOKEN_TOUCH_SQL, bind: [1, s, 0] },
  { name: "desktops: list", sql: LIST_DESKTOPS_SQL, bind: [s] },
  { name: "desktops: pending counts", sql: PENDING_COUNTS_SQL, bind: [s] },
  { name: "desktops: pending count of one", sql: PENDING_COUNT_ONE_SQL, bind: [s] },
  { name: "desktops: get", sql: GET_DESKTOP_SQL, bind: [s] },
  { name: "desktops: patch", sql: PATCH_DESKTOP_SQL, bind: [s, 0, null, 0, null, 0, null, 0, null, 0, null, s, s] },
  { name: "desktops: avatar image", sql: GET_AVATAR_SQL, bind: [s, s] },
  { name: "desktops: revoke tokens", sql: REVOKE_DESKTOP_TOKENS_SQL, bind: [1, s, s, s] },
  { name: "desktops: revoke grants", sql: REVOKE_DESKTOP_GRANTS_SQL, bind: [1, s, s, s] },
  { name: "desktops: revoke", sql: REVOKE_DESKTOP_SQL, bind: [1, s, s] },
  { name: "desktop/me: identity", sql: DESKTOP_ME_SQL, bind: [s] },
  { name: "desktop/me: unpair tokens", sql: UNPAIR_TOKENS_SQL, bind: [1, s] },
  { name: "devices: find by key", sql: FIND_DEVICE_SQL, bind: [s, s] },
  { name: "devices: insert (quota)", sql: INSERT_DEVICE_SQL, bind: [s, s, s, s, s, 1, s, 10, s, s] },
  { name: "devices: list", sql: LIST_DEVICES_SQL, bind: [s] },
  { name: "devices: patch", sql: PATCH_DEVICE_SQL, bind: [s, s, s] },
  { name: "devices: grants of a device", sql: DEVICE_GRANTS_SQL, bind: [s] },
  { name: "devices: forget grants", sql: FORGET_GRANTS_SQL, bind: [1, s, s, s] },
  { name: "devices: forget push subscriptions", sql: FORGET_PUSH_SQL, bind: [s, s, s] },
  { name: "devices: forget", sql: FORGET_DEVICE_SQL, bind: [1, s, s] },
  { name: "grants: own desktop", sql: OWN_DESKTOP_SQL, bind: [s, s] },
  { name: "grants: own device", sql: OWN_DEVICE_SQL, bind: [s, s] },
  { name: "grants: get", sql: GET_GRANT_SQL, bind: [s, s] },
  { name: "grants: list", sql: LIST_GRANTS_SQL, bind: [s] },
  { name: "grants: insert", sql: INSERT_GRANT_SQL, bind: [s, s, 1, s, s, s, s] },
  { name: "grants: request permission", sql: REQUEST_PERMISSION_SQL, bind: [s, 1, s, s] },
  { name: "grants: revoke", sql: REVOKE_GRANT_SQL, bind: [1, s, s] },
  { name: "desktop/grants: list", sql: DESKTOP_GRANTS_SQL, bind: [s] },
  { name: "desktop/grants: state of one", sql: DESKTOP_GRANT_STATE_SQL, bind: [s, s] },
  { name: "desktop/grants: confirm", sql: CONFIRM_GRANT_SQL, bind: ["read", 1, s, s] },
  { name: "desktop/grants: pending", sql: PENDING_GRANT_SQL, bind: [1, s, s] },
  { name: "grants: control grant of own device", sql: CONTROL_GRANT_SQL, bind: [s, s, s] },
  { name: "grants: insert selection ticket", sql: INSERT_SELECTION_TICKET_SQL, bind: [s, s, s, s, 1, 2] },
  { name: "desktop/grants: consume selection ticket", sql: CONSUME_SELECTION_TICKET_SQL, bind: [1, s, s, s] },
  { name: "grants: insert terminal ticket", sql: INSERT_TERMINAL_TICKET_SQL, bind: [s, s, s, s, s, 1, 2] },
  { name: "desktop/grants: consume terminal ticket", sql: CONSUME_TERMINAL_TICKET_SQL, bind: [1, s, s, s, s] },
  { name: "cleanup: pending grants", sql: (GRANT_CLEANUP[0] as { sql: string }).sql, bind: [1] },
];

describe("relay API query plans", () => {
  it.each(CASES.map((c) => [c.name, c] as const))("%s: no full table scan", async (_name, c) => {
    const { results } = await env.DB.prepare(`EXPLAIN QUERY PLAN ${c.sql}`).bind(...c.bind).all<{ detail: string }>();
    const details = results.map((r) => r.detail);
    const isInsert = /^\s*INSERT/i.test(c.sql);
    if (!isInsert) expect(details.length).toBeGreaterThan(0);
    for (const detail of details) {
      if (detail === "SCAN CONSTANT ROW") continue;
      expect(detail, detail).not.toMatch(/^SCAN /);
    }
    if (!isInsert) expect(details.some((d) => /^SEARCH .* USING (COVERING )?INDEX/.test(d)), details.join(" | ")).toBe(true);
  });

  it("the deny-approved updates use pairing_requests_desktop_id_idx", async () => {
    for (const [sql, bind] of [
      [DENY_OTHER_APPROVED_SQL, [s, s, s, s]],
      [UNPAIR_DENY_APPROVED_SQL, [s]],
      [DELETE_DENY_APPROVED_SQL, [s, s, s, 1]],
    ] as const) {
      const { results } = await env.DB.prepare(`EXPLAIN QUERY PLAN ${sql}`).bind(...bind).all<{ detail: string }>();
      expect(results.map((r) => r.detail).join(" | ")).toContain("pairing_requests_desktop_id_idx");
    }
  });

  it("the cleanup deletes use their indexes", async () => {
    for (const [sql, index] of [
      [(PAIRING_CLEANUP[0] as { sql: string }).sql, "pairing_requests_expires_at_idx"],
      [(GRANT_CLEANUP[0] as { sql: string }).sql, "device_grants_state_updated_at_idx"],
      [(GRANT_CLEANUP[1] as { sql: string }).sql, "selection_tickets_expires_at_idx"],
      [(GRANT_CLEANUP[2] as { sql: string }).sql, "terminal_tickets_expires_at_idx"],
    ] as const) {
      const { results } = await env.DB.prepare(`EXPLAIN QUERY PLAN ${sql}`).bind(1).all<{ detail: string }>();
      expect(results.map((r) => r.detail).join(" | ")).toContain(index);
    }
  });
});
