import { env } from "cloudflare:workers";
import { Hono } from "hono";
import { describe, expect, it } from "vitest";
import { ApiErrorSchema, API_ROUTES } from "@monoptah/protocol";
import type { AppEnv } from "../src/http.ts";
import { ipHash } from "../src/lib/ip.ts";
import { consume } from "../src/modules/ratelimit/limiter.ts";
import { consumeSession, MAX_TRACKED_SESSIONS, resetSessionLimits, SESSION_LIMIT, trackedSessions } from "../src/modules/ratelimit/memory.ts";
import { LIMITS, limitHandleAttempts, limitInviteAttempts } from "../src/modules/ratelimit/index.ts";
import { BROWSER_HEADERS, call } from "./helpers.ts";

let counter = 0;
/** A fresh fake client address per test, so tests never share a rate limit key. */
const freshIp = () => `203.0.113.${(counter += 1)}`;

const post = (path: string, ip: string, body: unknown = {}, extra: Record<string, string> = {}) =>
  call(path, { method: "POST", headers: { ...BROWSER_HEADERS, "CF-Connecting-IP": ip, ...extra }, body: JSON.stringify(body) });

async function expectLimited(res: Response, windowSeconds: number) {
  expect(res.status).toBe(429);
  expect(ApiErrorSchema.parse(await res.json()).code).toBe("rate_limited");
  const retryAfter = Number(res.headers.get("Retry-After"));
  expect(Number.isInteger(retryAfter)).toBe(true);
  expect(retryAfter).toBeGreaterThanOrEqual(1);
  expect(retryAfter).toBeLessThanOrEqual(windowSeconds);
}

async function breachRows(hash: string, group: string) {
  const { results } = await env.DB.prepare("SELECT meta, actor_kind FROM audit_log WHERE action = 'ratelimit.breach' AND json_extract(meta, '$.ipHash') = ? AND json_extract(meta, '$.group') = ?").bind(hash, group).all<{ meta: string; actor_kind: string }>();
  return results;
}

describe("the table of security.md 3.5", () => {
  it("has the documented limits", () => {
    expect(LIMITS.register).toEqual({ limit: 10, windowSeconds: 3600 });
    expect(LIMITS.login).toEqual({ limit: 30, windowSeconds: 600 });
    expect(LIMITS.recover).toEqual({ limit: 20, windowSeconds: 3600 });
    expect(LIMITS["pair-start"]).toEqual({ limit: 5, windowSeconds: 600 });
    expect(LIMITS["pair-poll"]).toEqual({ limit: 1, windowSeconds: 3 });
    expect(LIMITS["push-notify"]).toEqual({ limit: 30, windowSeconds: 60 });
    expect(LIMITS.invite.limit).toBe(5);
    expect(LIMITS.handle.limit).toBe(5);
    expect(SESSION_LIMIT).toBe(120);
  });

  it("every rateLimit group of the protocol registry has a limit", () => {
    for (const route of API_ROUTES) if (route.rateLimit) expect(LIMITS[route.rateLimit], route.rateLimit).toBeDefined();
  });
});

describe("consume (D1 fixed window)", () => {
  it("allows `limit` calls, refuses the next with the time left, and resets in the next window", async () => {
    const now = 1_900_000_000 - (1_900_000_000 % 600) + 100; // 100 s into a 600 s window
    const key = `test:direct:${freshIp()}`;
    for (let i = 1; i <= 3; i += 1) {
      const r = await consume(env, key, 3, 600, now);
      expect(r).toMatchObject({ ok: true, count: i, firstBreach: false });
    }
    const fourth = await consume(env, key, 3, 600, now);
    expect(fourth).toEqual({ ok: false, retryAfter: 500, count: 4, firstBreach: true });
    const fifth = await consume(env, key, 3, 600, now + 1);
    expect(fifth).toMatchObject({ ok: false, retryAfter: 499, firstBreach: false });
    const next = await consume(env, key, 3, 600, now + 500);
    expect(next).toMatchObject({ ok: true, count: 1 });
  });

  it("keeps keys independent", async () => {
    const a = `test:a:${freshIp()}`;
    const b = `test:b:${freshIp()}`;
    await consume(env, a, 1, 60);
    expect((await consume(env, a, 1, 60)).ok).toBe(false);
    expect((await consume(env, b, 1, 60)).ok).toBe(true);
  });

  it("uses one statement per call", async () => {
    // One INSERT ... ON CONFLICT ... RETURNING: after two calls the table holds one row with count 2.
    const key = `test:one:${freshIp()}`;
    await consume(env, key, 10, 60);
    await consume(env, key, 10, 60);
    const { results } = await env.DB.prepare("SELECT count FROM rate_limits WHERE key = ?").bind(key).all<{ count: number }>();
    expect(results).toEqual([{ count: 2 }]);
  });
});

describe("IP keyed groups", () => {
  it.each([
    ["register", "/api/auth/register/options", 10, 3600],
    ["login", "/api/auth/login/options", 30, 600],
    ["recover", "/api/auth/recover", 20, 3600],
    ["pair-start", "/api/pair/start", 5, 600],
  ] as const)("%s: call %i+1 is 429 with Retry-After, other IPs are unaffected, a new window resets", async (group, path, limit, windowSeconds) => {
    const ip = freshIp();
    for (let i = 0; i < limit; i += 1) expect((await post(path, ip)).status, `call ${i + 1}`).toBe(501);
    await expectLimited(await post(path, ip), windowSeconds);
    await expectLimited(await post(path, ip), windowSeconds);
    expect((await post(path, freshIp())).status).toBe(501);

    // Breach audited once per window (two refused calls above, one audit row), with the salted IP hash.
    const hash = await ipHash(env, ip);
    const rows = await breachRows(hash, group);
    expect(rows).toHaveLength(1);
    expect(rows[0]?.actor_kind).toBe("system");
    expect(JSON.parse(rows[0]?.meta ?? "{}")).toEqual({ group, limit, windowSeconds, ipHash: hash });

    // A new window: move the counter row back by one window.
    await env.DB.prepare("UPDATE rate_limits SET window_start = window_start - ? WHERE key = ?").bind(windowSeconds, `${group}:ip:${hash}`).run();
    expect((await post(path, ip)).status).toBe(501);
  });

  it("register and login: both routes of a group share one counter", async () => {
    const ip = freshIp();
    for (let i = 0; i < 5; i += 1) {
      expect((await post("/api/auth/register/options", ip)).status).toBe(501);
      expect((await post("/api/auth/register/verify", ip)).status).toBe(501);
    }
    await expectLimited(await post("/api/auth/register/verify", ip), 3600);
  });

  it("stores no raw IP in any rate limit key or audit row", async () => {
    const ip = freshIp();
    await post("/api/pair/start", ip);
    for (let i = 0; i < 6; i += 1) await post("/api/pair/start", ip);
    const keys = await env.DB.prepare("SELECT key FROM rate_limits WHERE key LIKE '%' || ? || '%'").bind(ip).all();
    expect(keys.results).toEqual([]);
    const audits = await env.DB.prepare("SELECT id FROM audit_log WHERE meta LIKE '%' || ? || '%'").bind(ip).all();
    expect(audits.results).toEqual([]);
    const hashed = await env.DB.prepare("SELECT key FROM rate_limits WHERE key = ?").bind(`pair-start:ip:${await ipHash(env, ip)}`).all();
    expect(hashed.results).toHaveLength(1);
  });

  it("the rate limit runs before the handler: a limited request never reaches it", async () => {
    const ip = freshIp();
    for (let i = 0; i < 5; i += 1) await post("/api/pair/start", ip);
    expect((await post("/api/pair/start", ip)).status).toBe(429);
  });

  it("does not limit other routes", async () => {
    const ip = freshIp();
    for (let i = 0; i < 15; i += 1) expect((await call("/api/health", { headers: { "CF-Connecting-IP": ip } })).status).toBe(200);
  });
});

describe("pair/poll: 1 per 3 s per deviceCode", () => {
  it("limits per device code, hashed in the key, at the middleware level", async () => {
    const ip = freshIp();
    const deviceCode = `device-code-${ip}`;
    expect((await post("/api/pair/poll", ip, { deviceCode })).status).toBe(501);
    await expectLimited(await post("/api/pair/poll", ip, { deviceCode }), 3);
    // Same IP, other device code: independent. Other IP, same device code: still limited (keyed by code).
    expect((await post("/api/pair/poll", ip, { deviceCode: `${deviceCode}-2` })).status).toBe(501);
    await expectLimited(await post("/api/pair/poll", freshIp(), { deviceCode }), 3);
    const keys = await env.DB.prepare("SELECT key FROM rate_limits WHERE key LIKE '%' || ? || '%'").bind(deviceCode).all();
    expect(keys.results).toEqual([]);
  });

  it("a new window resets it", async () => {
    const ip = freshIp();
    const deviceCode = `device-code-reset-${ip}`;
    await post("/api/pair/poll", ip, { deviceCode });
    expect((await post("/api/pair/poll", ip, { deviceCode })).status).toBe(429);
    await env.DB.prepare("UPDATE rate_limits SET window_start = window_start - 3 WHERE key LIKE 'pair-poll:device:%'").run();
    expect((await post("/api/pair/poll", ip, { deviceCode })).status).toBe(501);
  });

  it("leaves a body without a deviceCode to the handler", async () => {
    const ip = freshIp();
    for (let i = 0; i < 3; i += 1) expect((await post("/api/pair/poll", ip, {})).status).toBe(501);
    const bad = await call("/api/pair/poll", { method: "POST", headers: { ...BROWSER_HEADERS, "CF-Connecting-IP": ip }, body: "not json" });
    expect(bad.status).toBe(501);
  });
});

describe("push/notify: 30 per desktop token per minute", () => {
  it("limits per bearer token (hashed), not per IP", async () => {
    const ip = freshIp();
    const token = `desktop-token-${ip}`;
    const send = (t: string, from = ip) => post("/api/push/notify", from, {}, { Authorization: `Bearer ${t}` });
    for (let i = 0; i < 30; i += 1) expect((await send(token)).status, `call ${i + 1}`).toBe(501);
    await expectLimited(await send(token, freshIp()), 60);
    expect((await send(`${token}-other`)).status).toBe(501);
    const keys = await env.DB.prepare("SELECT key FROM rate_limits WHERE key LIKE '%' || ? || '%'").bind(token).all();
    expect(keys.results).toEqual([]);
  });
});

describe("limits that handlers call", () => {
  function app() {
    const a = new Hono<AppEnv>();
    a.post("/invite", async (c) => (await limitInviteAttempts(c, c.req.query("v") ?? "")) ?? c.json({ ok: true }));
    a.post("/handle", async (c) => (await limitHandleAttempts(c, c.req.query("v") ?? "")) ?? c.json({ ok: true }));
    return a;
  }
  const send = (path: string, value: string, ip: string) => app().request(`${path}?v=${encodeURIComponent(value)}`, { method: "POST", headers: { "CF-Connecting-IP": ip } }, env);

  it("invite code: 5 attempts per hour, hashed in the key", async () => {
    const code = `invite-code-${freshIp()}`;
    for (let i = 0; i < 5; i += 1) expect((await send("/invite", code, freshIp())).status).toBe(200);
    await expectLimited(await send("/invite", code, freshIp()), 3600);
    expect((await send("/invite", `${code}-b`, freshIp())).status).toBe(200);
    const keys = await env.DB.prepare("SELECT key FROM rate_limits WHERE key LIKE '%' || ? || '%'").bind(code).all();
    expect(keys.results).toEqual([]);
  });

  it("handle: 5 per hour, case-insensitive, hashed in the key", async () => {
    const handle = `someone${freshIp().replaceAll(".", "")}`;
    for (let i = 0; i < 5; i += 1) expect((await send("/handle", i % 2 ? handle.toUpperCase() : handle, freshIp())).status).toBe(200);
    await expectLimited(await send("/handle", handle, freshIp()), 3600);
    const keys = await env.DB.prepare("SELECT key FROM rate_limits WHERE key LIKE '%' || ? || '%'").bind(handle).all();
    expect(keys.results).toEqual([]);
  });
});

describe("the in-memory session limit", () => {
  it("allows 120 per minute, then refuses with the time left, once-audited flag, and resets next window", () => {
    resetSessionLimits();
    const now = 1_900_000_020 - (1_900_000_020 % 60) + 20;
    for (let i = 1; i <= SESSION_LIMIT; i += 1) expect(consumeSession("s1", now).ok).toBe(true);
    const over = consumeSession("s1", now);
    expect(over).toEqual({ ok: false, retryAfter: 40, firstBreach: true });
    expect(consumeSession("s1", now)).toMatchObject({ ok: false, firstBreach: false });
    expect(consumeSession("s2", now).ok).toBe(true);
    expect(consumeSession("s1", now + 40).ok).toBe(true);
  });

  it("stays bounded", () => {
    resetSessionLimits();
    const now = 1_900_000_000;
    for (let i = 0; i < MAX_TRACKED_SESSIONS + 50; i += 1) consumeSession(`bound-${i}`, now);
    expect(trackedSessions()).toBeLessThanOrEqual(MAX_TRACKED_SESSIONS);
    // Past windows are purged first.
    consumeSession("later", now + 120);
    expect(trackedSessions()).toBeLessThan(MAX_TRACKED_SESSIONS);
    resetSessionLimits();
  });
});
