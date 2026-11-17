// EXPLAIN QUERY PLAN of the push queries: none may scan a whole table.
import { env } from "cloudflare:workers";
import { describe, expect, it } from "vitest";
import { DESKTOP_PREFS_SQL, recipientsSql } from "../../src/modules/push/notify.ts";
import {
  DELETE_SUBSCRIPTION_SQL,
  OWN_DESKTOP_SQL,
  OWN_DEVICE_SQL,
  SUBSCRIPTION_ID_SQL,
  UPSERT_PREFS_SQL,
  UPSERT_SUBSCRIPTION_SQL,
} from "../../src/modules/push/subscriptions.ts";
import { BUMP_FAILURES_SQL, DELETE_SUBSCRIPTION_BY_ID_SQL, MARK_SUCCESS_SQL } from "../../src/modules/push/notify.ts";

const cases: { name: string; sql: string; bind: unknown[] }[] = [
  { name: "prefs lookup", sql: DESKTOP_PREFS_SQL, bind: ["d", "u"] },
  { name: "recipients", sql: recipientsSql(3), bind: ["d", "a", "b", "c", "u", "u"] },
  { name: "subscription by endpoint", sql: SUBSCRIPTION_ID_SQL, bind: ["e"] },
  { name: "own device", sql: OWN_DEVICE_SQL, bind: ["d", "u"] },
  { name: "own desktop", sql: OWN_DESKTOP_SQL, bind: ["d", "u"] },
  { name: "delete own subscription", sql: DELETE_SUBSCRIPTION_SQL, bind: ["i", "u"] },
  { name: "delete subscription by id", sql: DELETE_SUBSCRIPTION_BY_ID_SQL, bind: ["i"] },
  { name: "mark success", sql: MARK_SUCCESS_SQL, bind: [1, "i"] },
  { name: "bump failures", sql: BUMP_FAILURES_SQL, bind: ["i"] },
  { name: "upsert subscription", sql: UPSERT_SUBSCRIPTION_SQL, bind: ["i", "u", "d", "e", "p", "a", "k", null, 1, "d", "e"] },
  { name: "upsert prefs", sql: UPSERT_PREFS_SQL, bind: ["u", "d", 1, "{}"] },
];

describe("push query plans", () => {
  for (const c of cases) {
    it(`${c.name} does not scan a whole table`, async () => {
      const { results } = await env.DB.prepare(`EXPLAIN QUERY PLAN ${c.sql}`).bind(...c.bind).all<{ detail: string }>();
      const scans = results.map((r) => r.detail).filter((d) => /^SCAN /.test(d) && d !== "SCAN CONSTANT ROW" && !/USING (COVERING )?INDEX/.test(d));
      expect(scans, results.map((r) => r.detail).join(" | ")).toEqual([]);
    });
  }
});
