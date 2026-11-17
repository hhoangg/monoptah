// EXPLAIN QUERY PLAN of every query the accounts modules run per request: none may scan a whole table.
// Add each new query here (with the real SQL constant exported by its module), never weaken the check.
import { env } from "cloudflare:workers";
import { describe, expect, it } from "vitest";
import { CONSUME_SQL } from "../src/modules/ratelimit/limiter.ts";

interface PlanCase {
  name: string;
  sql: string;
  bind: unknown[];
  /** The index (or primary key) the plan must search. */
  uses: RegExp;
}

const CASES: PlanCase[] = [
  { name: "rate limit consume", sql: CONSUME_SQL, bind: ["k", 0], uses: /rate_limits|PRIMARY KEY|autoindex/ },
];

describe("query plans", () => {
  it.each(CASES.map((c) => [c.name, c] as const))("%s: no full table scan", async (_name, c) => {
    const { results } = await env.DB.prepare(`EXPLAIN QUERY PLAN ${c.sql}`).bind(...c.bind).all<{ detail: string }>();
    const details = results.map((r) => r.detail);
    const isInsert = /^\s*INSERT/i.test(c.sql);
    // An INSERT ... ON CONFLICT has no plan rows: its conflict target is the primary key (checked in schema.test.ts).
    if (!isInsert) expect(details.length).toBeGreaterThan(0);
    for (const detail of details) {
      // "SCAN table" without USING is a full scan (an INSERT has no plan row of its own, which is fine).
      expect(detail, detail).not.toMatch(/^SCAN (?!.*USING)/);
    }
    if (!isInsert) expect(details.join(" | "), details.join(" | ")).toMatch(c.uses);
  });
});
