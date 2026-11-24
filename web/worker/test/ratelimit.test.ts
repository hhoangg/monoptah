import { env } from "cloudflare:workers";
import { Hono } from "hono";
import { describe, expect, it } from "vitest";
import { ApiErrorSchema, API_ROUTES } from "@monoptah/protocol";
import type { AppEnv } from "../src/http.ts";
import { sha256Hex } from "../src/lib/crypto.ts";
import { ipHash } from "../src/lib/ip.ts";
import { consume, CONSUME_SQL } from "../src/modules/ratelimit/limiter.ts";
import { AVATAR_SESSION_LIMIT, consumeIp, consumeSession, MAX_TRACKED_IPS, MAX_TRACKED_SESSIONS, resetIpLimits, resetSessionLimits, SESSION_LIMIT, trackedIps, trackedSessions } from "../src/modules/ratelimit/memory.ts";
import { IP_LIMITS, LIMITS, limitHandleAttempts, limitOutstandingPairings } from "../src/modules/ratelimit/index.ts";
import { BROWSER_HEADERS, call } from "./helpers.ts";
import { authedCall, createTestInvite, createTestUser, loginAs, postJson, registerOptions } from "./support/accounts.ts";
import { VirtualAuthenticator } from "./support/authenticator.ts";

let counter = 0;
/** A fresh fake client address per test, so tests never share a rate limit key. */
const freshIp = () => `203.0.113.${(counter += 1)}`;

const post = (path: string, ip: string, body: unknown = {}, extra: Record<string, string> = {}) =>
  call(path, { method: "POST", headers: { ...BROWSER_HEADERS, "CF-Connecting-IP": ip, ...extra }, body: JSON.stringify(body) });

/**
 * Passes the limiter: the route answers with its own status, never 429 (limited), 403 (browser checks) or 501.
 * The pairing routes reached with an empty body answer their own validation error: 400 invalid.
 */
const PAIRING_PATHS = new Set(["/api/pair/start", "/api/pair/poll"]);
function expectPassed(res: Response, path: string, message?: string) {
  expect([429, 403, 501], message).not.toContain(res.status);
  if (PAIRING_PATHS.has(path)) expect(res.status, message).toBe(400);
}

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

  it("a refused call writes nothing: the first one writes count = limit + 1, later ones write no row", async () => {
    const key = `test:norefusedwrite:${freshIp()}`;
    const windowStart = 1_900_000_000 - (1_900_000_000 % 600);
    const run = async () => {
      const result = await env.DB.prepare(CONSUME_SQL).bind(key, windowStart, 3).run<{ count: number }>();
      return { rows: result.results.length, written: result.meta.rows_written };
    };
    for (let i = 0; i < 3; i += 1) expect((await run()).written).toBeGreaterThan(0);
    const first = await run(); // count 3 -> 4: the audited breach, one write
    expect(first.rows).toBe(1);
    expect(first.written).toBeGreaterThan(0);
    for (let i = 0; i < 5; i += 1) expect(await run(), `refused call ${i + 1}`).toEqual({ rows: 0, written: 0 });
    const { results } = await env.DB.prepare("SELECT count FROM rate_limits WHERE key = ?").bind(key).all<{ count: number }>();
    expect(results).toEqual([{ count: 4 }]);
  });

  it("consume keeps answering 'limited' (count = limit + 1, no second firstBreach) after the counter stops moving", async () => {
    const key = `test:stilllimited:${freshIp()}`;
    const now = 1_900_000_000 - (1_900_000_000 % 600) + 10;
    for (let i = 0; i < 2; i += 1) await consume(env, key, 2, 600, now);
    expect(await consume(env, key, 2, 600, now)).toMatchObject({ ok: false, count: 3, firstBreach: true });
    for (let i = 0; i < 3; i += 1) expect(await consume(env, key, 2, 600, now)).toMatchObject({ ok: false, count: 3, firstBreach: false });
    const { results } = await env.DB.prepare("SELECT count FROM rate_limits WHERE key = ?").bind(key).all<{ count: number }>();
    expect(results).toEqual([{ count: 3 }]);
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
    for (let i = 0; i < limit; i += 1) expectPassed(await post(path, ip), path, `call ${i + 1}`);
    await expectLimited(await post(path, ip), windowSeconds);
    await expectLimited(await post(path, ip), windowSeconds);
    expectPassed(await post(path, freshIp()), path);

    // Breach audited once per window (two refused calls above, one audit row), with the salted IP hash.
    const hash = await ipHash(env, ip);
    const rows = await breachRows(hash, group);
    expect(rows).toHaveLength(1);
    expect(rows[0]?.actor_kind).toBe("system");
    expect(JSON.parse(rows[0]?.meta ?? "{}")).toEqual({ group, limit, windowSeconds, ipHash: hash });

    // A new window: move the counter row back by one window.
    await env.DB.prepare("UPDATE rate_limits SET window_start = window_start - ? WHERE key = ?").bind(windowSeconds, `${group}:ip:${hash}`).run();
    resetIpLimits(); // the in-memory pre-filter has its own window
    expectPassed(await post(path, ip), path);
  });

  it("register and login: both routes of a group share one counter", async () => {
    const ip = freshIp();
    for (let i = 0; i < 5; i += 1) {
      expectPassed(await post("/api/auth/register/options", ip), "/api/auth/register/options");
      expectPassed(await post("/api/auth/register/verify", ip), "/api/auth/register/verify");
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
    expect((await post("/api/pair/poll", ip, { deviceCode })).status).toBe(400);
    await expectLimited(await post("/api/pair/poll", ip, { deviceCode }), 3);
    // Same IP, other device code: independent. Other IP, same device code: still limited (keyed by code).
    expect((await post("/api/pair/poll", ip, { deviceCode: `${deviceCode}-2` })).status).toBe(400);
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
    expect((await post("/api/pair/poll", ip, { deviceCode })).status).toBe(400);
  });

  it("leaves a body without a deviceCode to the handler", async () => {
    const ip = freshIp();
    for (let i = 0; i < 3; i += 1) expect((await post("/api/pair/poll", ip, {})).status).toBe(400);
    const bad = await call("/api/pair/poll", { method: "POST", headers: { ...BROWSER_HEADERS, "CF-Connecting-IP": ip }, body: "not json" });
    expect(bad.status).toBe(400);
  });
});

describe("push/notify: 30 per desktop token per minute", () => {
  it("limits per bearer token (hashed), not per IP", async () => {
    const ip = freshIp();
    const token = `desktop-token-${ip}`;
    const send = (t: string, from = ip) => post("/api/push/notify", from, {}, { Authorization: `Bearer ${t}` });
    for (let i = 0; i < 30; i += 1) expect((await send(token)).status, `call ${i + 1}`).toBe(401);
    await expectLimited(await send(token, freshIp()), 60);
    expect((await send(`${token}-other`)).status).toBe(401);
    const keys = await env.DB.prepare("SELECT key FROM rate_limits WHERE key LIKE '%' || ? || '%'").bind(token).all();
    expect(keys.results).toEqual([]);
  });
});

describe("the in-memory pre-filter in front of the D1 per-IP limits", () => {
  it("answers 429 without touching D1 once the isolate has seen the IP over its limit", async () => {
    const ip = freshIp();
    const hash = await ipHash(env, ip);
    const key = `pair-start:ip:${hash}`;
    for (let i = 0; i < 5; i += 1) expectPassed(await post("/api/pair/start", ip), "/api/pair/start");
    await expectLimited(await post("/api/pair/start", ip), 600); // the breach call reaches D1 (audited once)
    // Forget D1 completely: a refusal that still comes back is the in-memory pre-filter, and D1 stays untouched.
    await env.DB.prepare("DELETE FROM rate_limits WHERE key = ?").bind(key).run();
    await expectLimited(await post("/api/pair/start", ip), 600);
    await expectLimited(await post("/api/pair/start", ip), 600);
    const rows = await env.DB.prepare("SELECT count FROM rate_limits WHERE key = ?").bind(key).all();
    expect(rows.results).toEqual([]);
    expect(await breachRows(hash, "pair-start")).toHaveLength(1);
  });

  it("uses the limit and window of the D1 per-IP limit, per group and per IP", () => {
    resetIpLimits();
    const now = 1_900_000_000 - (1_900_000_000 % 60) + 5;
    for (let i = 0; i < 120; i += 1) expect(consumeIp("pair-poll:ip:h1", 120, 60, now).ok).toBe(true);
    expect(consumeIp("pair-poll:ip:h1", 120, 60, now)).toEqual({ ok: false, retryAfter: 55, firstBreach: true });
    expect(consumeIp("pair-poll:ip:h1", 120, 60, now)).toMatchObject({ ok: false, firstBreach: false });
    expect(consumeIp("pair-poll:ip:h2", 120, 60, now).ok).toBe(true);
    expect(consumeIp("push-notify:ip:h1", 120, 60, now).ok).toBe(true);
    expect(consumeIp("pair-poll:ip:h1", 120, 60, now + 55).ok).toBe(true);
    resetIpLimits();
  });

  it("keeps live long-window counters when a short-window key is inserted into a full table", () => {
    resetIpLimits();
    const now = 1_900_000_000 - (1_900_000_000 % 3600) + 5;
    for (let i = 0; i < MAX_TRACKED_IPS; i += 1) consumeIp(`register:ip:long-${i}`, 10, 3600, now);
    expect(trackedIps()).toBe(MAX_TRACKED_IPS);
    // A 60 s key in a later minute of the same hour: the 3600 s windows are still live and must not be purged.
    consumeIp("pair-poll:ip:short", 120, 60, now + 120);
    // The live counters survive (the table is full of live entries, so only the oldest one is trimmed): the newest
    // long key still carries its first call, so the 10th call is the last allowed.
    for (let i = 0; i < 9; i += 1) consumeIp(`register:ip:long-${MAX_TRACKED_IPS - 1}`, 10, 3600, now + 120);
    expect(consumeIp(`register:ip:long-${MAX_TRACKED_IPS - 1}`, 10, 3600, now + 120)).toMatchObject({ ok: false });
    expect(trackedIps()).toBeLessThanOrEqual(MAX_TRACKED_IPS);
    resetIpLimits();
  });

  it("purges counters whose own window has ended when the table is full", () => {
    resetIpLimits();
    const now = 1_900_000_000 - (1_900_000_000 % 3600) + 5;
    for (let i = 0; i < MAX_TRACKED_IPS; i += 1) consumeIp(`pair-poll:ip:old-${i}`, 120, 60, now);
    consumeIp("register:ip:new", 10, 3600, now + 120);
    expect(trackedIps()).toBe(1);
    resetIpLimits();
  });

  it("stays bounded", () => {
    resetIpLimits();
    const now = 1_900_000_000;
    for (let i = 0; i < MAX_TRACKED_IPS + 50; i += 1) consumeIp(`register:ip:bound-${i}`, 10, 3600, now);
    expect(trackedIps()).toBeLessThanOrEqual(MAX_TRACKED_IPS);
    resetIpLimits();
  });
});

describe("per-IP bound of pair/poll and push/notify (attacker-chosen keys)", () => {
  it("has the documented numbers", () => {
    expect(IP_LIMITS["pair-poll"]).toEqual({ limit: 120, windowSeconds: 60 });
    expect(IP_LIMITS["push-notify"]).toEqual({ limit: 120, windowSeconds: 60 });
  });

  it("pair/poll: 120 calls per IP per minute whatever the device codes, checked before the per-code key", async () => {
    const ip = freshIp();
    for (let i = 0; i < 120; i += 1) expect((await post("/api/pair/poll", ip, { deviceCode: `dc-${ip}-${i}` })).status, `call ${i + 1}`).toBe(400);
    await expectLimited(await post("/api/pair/poll", ip, { deviceCode: `dc-${ip}-fresh` }), 60);
    // The per-code key of the refused call was never consumed.
    const fresh = await env.DB.prepare("SELECT 1 FROM rate_limits WHERE key = ?").bind(`pair-poll:device:${await sha256Hex(`dc-${ip}-fresh`)}`).first();
    expect(fresh).toBeNull();
    expect((await post("/api/pair/poll", freshIp(), { deviceCode: `dc-other-${ip}` })).status).toBe(400);
  });

  it("push/notify: 120 calls per IP per minute whatever the tokens, also without a token", async () => {
    const ip = freshIp();
    for (let i = 0; i < 120; i += 1) expect((await post("/api/push/notify", ip, {}, { Authorization: `Bearer tok-${ip}-${i}` })).status, `call ${i + 1}`).toBe(401);
    await expectLimited(await post("/api/push/notify", ip, {}, { Authorization: `Bearer tok-${ip}-fresh` }), 60);
    await expectLimited(await post("/api/push/notify", ip, {}), 60);
    const fresh = await env.DB.prepare("SELECT 1 FROM rate_limits WHERE key = ?").bind(`push-notify:token:${await sha256Hex(`tok-${ip}-fresh`)}`).first();
    expect(fresh).toBeNull();
  });
});

describe("IPv6 clients are limited per /64", () => {
  it("two addresses of one /64 share the counter, another /64 does not", async () => {
    const prefix = `2001:db8:${(counter += 1).toString(16)}:7`;
    for (let i = 0; i < 5; i += 1) expectPassed(await post("/api/pair/start", `${prefix}::${i + 1}`), "/api/pair/start");
    await expectLimited(await post("/api/pair/start", `${prefix}:aaaa:bbbb:cccc:dddd`.toUpperCase()), 600);
    expectPassed(await post("/api/pair/start", `2001:db8:${(counter += 1).toString(16)}:7::1`), "/api/pair/start");
  });
});

describe("limitOutstandingPairings (3 outstanding per IP)", () => {
  const app = () => {
    const a = new Hono<AppEnv>();
    a.post("/start", async (c) => (await limitOutstandingPairings(c, 1_000)) ?? c.json({ ok: true }));
    return a;
  };
  const start = (ip: string) => app().request("/start", { method: "POST", headers: { "CF-Connecting-IP": ip } }, env);
  let n = 0;
  const insert = async (hash: string, status: string, expiresAt: number) => {
    n += 1;
    await env.DB.prepare(
      "INSERT INTO pairing_requests (id, user_code, device_code_hash, challenge, desktop_public_key, machine_name, confirmation_code, status, created_at, expires_at, ip_hash) VALUES (?, ?, ?, 'c', 'k', 'm', '000000', ?, 1, ?, ?)",
    ).bind(`po-${hash}-${n}`, `PO${hash.slice(0, 6)}${n}`, `po-dc-${hash}-${n}`, status, expiresAt, hash).run();
  };

  it("allows up to 2 outstanding requests and refuses at 3 with Retry-After", async () => {
    const ip = freshIp();
    const hash = await ipHash(env, ip);
    expect((await start(ip)).status).toBe(200);
    await insert(hash, "pending", 2_000);
    await insert(hash, "pending", 2_000);
    expect((await start(ip)).status).toBe(200);
    await insert(hash, "pending", 2_000);
    const res = await start(ip);
    expect(res.status).toBe(429);
    expect(ApiErrorSchema.parse(await res.json()).code).toBe("rate_limited");
    expect(Number(res.headers.get("Retry-After"))).toBeGreaterThanOrEqual(1);
  });

  it("does not count expired, approved, denied, consumed or other IPs' requests", async () => {
    const ip = freshIp();
    const hash = await ipHash(env, ip);
    await insert(hash, "pending", 1_000); // expires_at = now: not outstanding
    await insert(hash, "pending", 500);
    await insert(hash, "approved", 2_000);
    await insert(hash, "denied", 2_000);
    await insert(hash, "consumed", 2_000);
    await insert(hash, "expired", 2_000);
    await insert(await ipHash(env, freshIp()), "pending", 2_000);
    expect((await start(ip)).status).toBe(200);
  });

  it("counts an IPv6 client by its /64", async () => {
    const prefix = `2001:db8:${(counter += 1).toString(16)}:9`;
    const hash = await ipHash(env, `${prefix}::1`);
    for (let i = 0; i < 3; i += 1) await insert(hash, "pending", 2_000);
    expect((await start(`${prefix}:1:2:3:4`)).status).toBe(429);
  });
});

describe("limits that handlers call", () => {
  function app() {
    const a = new Hono<AppEnv>();
        a.post("/handle", async (c) => (await limitHandleAttempts(c, c.req.query("v") ?? "")) ?? c.json({ ok: true }));
    return a;
  }
  const send = (path: string, value: string, ip: string) => app().request(`${path}?v=${encodeURIComponent(value)}`, { method: "POST", headers: { "CF-Connecting-IP": ip } }, env);

  it("handle: 5 per hour, case-insensitive, hashed in the key", async () => {
    const handle = `someone${freshIp().replaceAll(".", "")}`;
    for (let i = 0; i < 5; i += 1) expect((await send("/handle", i % 2 ? handle.toUpperCase() : handle, freshIp())).status).toBe(200);
    await expectLimited(await send("/handle", handle, freshIp()), 3600);
    const keys = await env.DB.prepare("SELECT key FROM rate_limits WHERE key LIKE '%' || ? || '%'").bind(handle).all();
    expect(keys.results).toEqual([]);
  });
});

describe("the per-invite-code limit on both registration routes", () => {
  it("5 attempts per hour over register/options and register/verify together, hashed in the key; the 6th is 429 on either route", async () => {
    const invite = await createTestInvite(env);
    const key = await VirtualAuthenticator.generate();
    const attempts = [];
    for (let i = 0; i < 3; i += 1) {
      const attempt = await registerOptions(invite.code, `inv-${crypto.randomUUID().slice(0, 8)}`); // attempts 1 to 3
      attempts.push({ challengeId: attempt.challengeId, response: await key.create(attempt.options, { origin: env.RP_ORIGIN, rpId: env.RP_ID }) });
    }
    // Attempts 4 and 5 are verify calls (the first registers, the second finds the invite used): both count.
    expect((await postJson("/api/auth/register/verify", attempts[0])).status).toBe(200);
    expect((await postJson("/api/auth/register/verify", attempts[1])).status).toBe(400);
    const keys = await env.DB.prepare("SELECT key, count FROM rate_limits WHERE key LIKE 'invite:code:%' AND count = 5").all<{ key: string }>();
    expect(keys.results).toHaveLength(1);
    expect(keys.results[0]?.key).not.toContain(invite.code);
    // Attempt 6: the same 429 on verify and on options.
    await expectLimited(await postJson("/api/auth/register/verify", attempts[2]), 3600);
    await expectLimited(await postJson("/api/auth/register/options", { invite: invite.code, handle: "someone-else", displayName: "X" }), 3600);
    // Another code is unaffected.
    const other = await createTestInvite(env);
    expect((await postJson("/api/auth/register/options", { invite: other.code, handle: `inv-${crypto.randomUUID().slice(0, 8)}`, displayName: "X" })).status).toBe(200);
  });
});

describe("avatar limit: 300 per session per minute, separate from the general one", () => {
  const missing = () => `/api/desktops/${crypto.randomUUID()}/avatar`;

  it("does not charge the general bucket, and normal calls do not charge the avatar bucket", async () => {
    resetSessionLimits();
    const session = await loginAs((await createTestUser(env)).id);
    for (let i = 0; i < SESSION_LIMIT + 10; i += 1) expect((await authedCall(session, missing())).status, `avatar ${i + 1}`).toBe(404);
    for (let i = 0; i < SESSION_LIMIT; i += 1) expect((await authedCall(session, "/api/me")).status, `me ${i + 1}`).toBe(200);
    await expectLimited(await authedCall(session, "/api/me"), 60);
    // The general bucket is spent; avatars still have their own.
    expect((await authedCall(session, missing())).status).toBe(404);
    resetSessionLimits();
  });

  it("answers 429 with Retry-After after 300 avatar GETs and audits the first breach once", async () => {
    resetSessionLimits();
    const user = await createTestUser(env);
    const session = await loginAs(user.id);
    for (let i = 0; i < AVATAR_SESSION_LIMIT; i += 1) expect((await authedCall(session, missing())).status, `call ${i + 1}`).toBe(404);
    const over = await authedCall(session, missing());
    await expectLimited(over, 60);
    await expectLimited(await authedCall(session, missing()), 60);
    // The general bucket is untouched.
    expect((await authedCall(session, "/api/me")).status).toBe(200);
    const { results } = await env.DB.prepare("SELECT meta FROM audit_log WHERE action = 'ratelimit.breach' AND actor_user_id = ?").bind(user.id).all<{ meta: string }>();
    expect(results).toHaveLength(1);
    expect(JSON.parse(results[0]?.meta ?? "{}")).toMatchObject({ group: "session-avatar", limit: 300, windowSeconds: 60 });
    resetSessionLimits();
  });

  it("still answers 401 without a session before any counting", async () => {
    expect((await authedCall({ cookie: "" }, missing())).status).toBe(401);
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
