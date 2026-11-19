import { env } from "cloudflare:workers";
import { createExecutionContext, createScheduledController, waitOnExecutionContext } from "cloudflare:test";
import { beforeEach, describe, expect, it } from "vitest";
import { runCleanups } from "../src/cron.ts";
import worker from "../src/index.ts";
import { MODULES } from "../src/modules/index.ts";
import { DAY, HOUR, RETENTION, type CleanupQuery } from "../src/modules/cleanup.ts";
import { CLEANUP_QUERIES as audit } from "../src/modules/audit/cleanup.ts";
import { CLEANUP_QUERIES as auth } from "../src/modules/auth/cleanup.ts";
import { CLEANUP_QUERIES as ratelimit } from "../src/modules/ratelimit/cleanup.ts";
import { CLEANUP_QUERIES as sessions } from "../src/modules/sessions/cleanup.ts";

const NOW = 1_900_000_000;
const ALL_QUERIES: readonly CleanupQuery[] = [...auth, ...sessions, ...ratelimit, ...audit];

describe("cleanup queries use an index", () => {
  it("covers the four WS-A cleanups", () => {
    expect(ALL_QUERIES.map((q) => q.name).sort()).toEqual(["audit_log", "challenges", "rate_limits", "sessions"]);
  });

  it.each(ALL_QUERIES.map((q) => [q.name, q] as const))("%s: EXPLAIN QUERY PLAN has no full table scan", async (_name, query) => {
    const { results } = await env.DB.prepare(`EXPLAIN QUERY PLAN ${query.sql}`).bind(...query.bind(NOW)).all<{ detail: string }>();
    const details = results.map((r) => r.detail);
    expect(details.length).toBeGreaterThan(0);
    // "SCAN table" without USING INDEX is a full scan; "SEARCH table USING INDEX" is what we want.
    for (const detail of details) expect(detail, detail).not.toMatch(/^SCAN (?!.*USING)/);
    expect(details.some((d) => /SEARCH .* USING (COVERING )?INDEX/.test(d)), details.join(" | ")).toBe(true);
  });
});

async function seed() {
  const db = env.DB;
  await db.batch([
    db.prepare("DELETE FROM challenges"),
    db.prepare("DELETE FROM sessions"),
    db.prepare("DELETE FROM rate_limits"),
    db.prepare("DELETE FROM audit_log"),
    db.prepare("DELETE FROM users WHERE id = 'cron-user'"),
    db.prepare("INSERT INTO users (id, handle, display_name, created_at) VALUES ('cron-user', 'cron-user', 'Cron', 1)"),
  ]);
  const challenge = (id: string, expiresAt: number) =>
    db.prepare("INSERT INTO challenges (id, purpose, challenge, expires_at) VALUES (?, 'login', 'c', ?)").bind(id, expiresAt);
  const session = (id: string, idleExpiresAt: number, expiresAt: number) =>
    db.prepare("INSERT INTO sessions (id_hash, user_id, created_at, last_seen_at, idle_expires_at, expires_at, amr) VALUES (?, 'cron-user', 1, 1, ?, ?, 'passkey')").bind(id, idleExpiresAt, expiresAt);
  const rate = (key: string, windowStart: number) => db.prepare("INSERT INTO rate_limits (key, window_start, count) VALUES (?, ?, 1)").bind(key, windowStart);
  const audit = (id: string, ts: number) => db.prepare("INSERT INTO audit_log (id, ts, actor_kind, action) VALUES (?, ?, 'system', 'test')").bind(id, ts);
  await db.batch([
    // challenges: expired long ago (delete), expired a minute ago (inside the 1 h grace), still valid
    challenge("ch-old", NOW - HOUR - 1),
    challenge("ch-just-expired", NOW - 60),
    challenge("ch-valid", NOW + 300),
    // sessions: absolute expiry long past, idle expiry long past, both in the future, expired inside the 7 day grace
    session("se-abs-old", NOW + DAY, NOW - 7 * DAY - 1),
    session("se-idle-old", NOW - 7 * DAY - 1, NOW + DAY),
    session("se-live", NOW + 7 * DAY, NOW + 30 * DAY),
    session("se-in-grace", NOW - DAY, NOW - DAY),
    // rate limit windows
    rate("rl-old", NOW - DAY - 1),
    rate("rl-recent", NOW - 60),
    // audit rows
    audit("au-old", NOW - 180 * DAY - 1),
    audit("au-fresh", NOW - 179 * DAY),
  ]);
}

const ids = async (table: string, column: string) => {
  const { results } = await env.DB.prepare(`SELECT ${column} AS id FROM ${table} ORDER BY ${column}`).all<{ id: string }>();
  return results.map((r) => r.id);
};

describe("daily cleanup", () => {
  beforeEach(seed);

  it("retention constants follow security.md 10", () => {
    expect(RETENTION).toEqual({ challenges: 3600, sessions: 7 * 86400, rateLimits: 86400, auditLog: 180 * 86400, selectionTickets: 3600 });
  });

  it("runCleanups deletes expired rows and keeps fresh ones", async () => {
    await runCleanups(env, NOW);
    expect(await ids("challenges", "id")).toEqual(["ch-just-expired", "ch-valid"]);
    expect(await ids("sessions", "id_hash")).toEqual(["se-in-grace", "se-live"]);
    expect(await ids("rate_limits", "key")).toEqual(["rl-recent"]);
    expect(await ids("audit_log", "id")).toEqual(["au-fresh"]);
  });

  it("is idempotent", async () => {
    await runCleanups(env, NOW);
    await runCleanups(env, NOW);
    expect(await ids("audit_log", "id")).toEqual(["au-fresh"]);
  });

  it("every module exports a cleanup and the stubs do not touch rows", async () => {
    for (const [name, entry] of Object.entries(MODULES)) expect(typeof entry.cleanup, name).toBe("function");
    await MODULES.pairing.cleanup(env, NOW);
    await MODULES.grants.cleanup(env, NOW);
    await MODULES.push.cleanup(env, NOW);
    expect((await ids("challenges", "id")).length).toBe(3);
  });

  it("the scheduled handler runs the cleanup at the scheduled time", async () => {
    const ctx = createExecutionContext();
    await worker.scheduled(createScheduledController({ scheduledTime: NOW * 1000, cron: "17 3 * * *" }), env, ctx);
    await waitOnExecutionContext(ctx);
    expect(await ids("challenges", "id")).toEqual(["ch-just-expired", "ch-valid"]);
    expect(await ids("audit_log", "id")).toEqual(["au-fresh"]);
  });
});
