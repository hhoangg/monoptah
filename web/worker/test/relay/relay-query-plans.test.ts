import { env } from "cloudflare:workers";
import { describe, expect, it } from "vitest";
import { DEVICE_JOIN_SQL } from "../../src/modules/relay/index.ts";

describe("relay query plans", () => {
  it("the device join check has no full table scan", async () => {
    const { results } = await env.DB.prepare(`EXPLAIN QUERY PLAN ${DEVICE_JOIN_SQL}`).bind("d", "k", "u", "u").all<{ detail: string }>();
    expect(results.length).toBeGreaterThan(0);
    for (const row of results) expect(row.detail, row.detail).not.toMatch(/^SCAN /);
  });
});
