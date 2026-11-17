import { env } from "cloudflare:workers";
import { Hono } from "hono";
import { beforeEach, describe, expect, it } from "vitest";
import { ApiErrorSchema, findRoute, SessionsResponse, type ApiRoute } from "@monoptah/protocol";
import type { AppEnv } from "../src/http.ts";
import { sha256Hex } from "../src/lib/crypto.ts";
import { authResolver, isFresh, requireFresh } from "../src/middleware/auth.ts";
import { resetSessionLimits, SESSION_LIMIT } from "../src/modules/ratelimit/memory.ts";
import {
  ABSOLUTE_SECONDS,
  clearSessionCookie,
  closeSessionSockets,
  createSession,
  deleteAllSessionsStatement,
  IDLE_SECONDS,
  listSessionIdHashes,
  MAX_REVOKE_CALLS,
  RECOVERY_ONLY_SECONDS,
  revokeAllSessions,
  revokeSessions,
  sessionCookie,
  SESSION_COOKIE,
} from "../src/modules/sessions/session.ts";
import { call } from "./helpers.ts";
import { authedCall, createTestDesktop, createTestUser, fakeRoomNamespace, loginAs, setCookies, withRoom } from "./support/accounts.ts";

const now = () => Math.floor(Date.now() / 1000);

/** Parses a Set-Cookie value into its name, value and attribute set (lowercase names). */
function parseSetCookie(header: string) {
  const [pair, ...attributes] = header.split(";").map((part) => part.trim());
  const eq = (pair ?? "").indexOf("=");
  return {
    name: (pair ?? "").slice(0, eq),
    value: (pair ?? "").slice(eq + 1),
    attributes: new Map(attributes.map((a) => { const i = a.indexOf("="); return i < 0 ? [a.toLowerCase(), ""] as const : [a.slice(0, i).toLowerCase(), a.slice(i + 1)] as const; })),
  };
}

async function sessionRow(idHash: string) {
  return env.DB.prepare("SELECT * FROM sessions WHERE id_hash = ?").bind(idHash).first<Record<string, number | string | null>>();
}

async function expectError(res: Response, status: number, code: string) {
  expect(res.status).toBe(status);
  expect(ApiErrorSchema.parse(await res.json()).code).toBe(code);
}

beforeEach(() => resetSessionLimits());

describe("session cookie", () => {
  it("has the attributes of security.md 3.3: __Host- name, HttpOnly, Secure, Path=/, SameSite=Strict, no Domain", async () => {
    const user = await createTestUser(env);
    const created = await createSession(env, { userId: user.id, amr: "passkey" });
    const cookie = parseSetCookie(created.cookie);
    expect(cookie.name).toBe("__Host-mc_session");
    expect(cookie.name).toBe(SESSION_COOKIE);
    expect(cookie.value).toBe(created.token);
    expect(cookie.attributes.has("httponly")).toBe(true);
    expect(cookie.attributes.has("secure")).toBe(true);
    expect(cookie.attributes.get("path")).toBe("/");
    expect(cookie.attributes.get("samesite")).toBe("Strict");
    expect(cookie.attributes.has("domain")).toBe(false);
    expect(cookie.attributes.get("max-age")).toBe(String(ABSOLUTE_SECONDS));
  });

  it("sessionCookie and clearSessionCookie build the same attributes, and the clear value expires at once", () => {
    expect(sessionCookie("tok", 100)).toBe("__Host-mc_session=tok; HttpOnly; Secure; Path=/; SameSite=Strict; Max-Age=100");
    const cleared = parseSetCookie(clearSessionCookie());
    expect(cleared.value).toBe("");
    expect(cleared.attributes.get("max-age")).toBe("0");
    expect(cleared.attributes.has("domain")).toBe(false);
    expect(cleared.attributes.has("httponly")).toBe(true);
  });

  it("logout sends the clearing cookie with the same attributes", async () => {
    const session = await loginAs((await createTestUser(env)).id);
    const res = await authedCall(session, "/api/auth/logout", { method: "POST" });
    const [header] = setCookies(res);
    const cookie = parseSetCookie(header ?? "");
    expect(cookie.name).toBe("__Host-mc_session");
    expect(cookie.value).toBe("");
    expect(cookie.attributes.get("max-age")).toBe("0");
    expect(cookie.attributes.get("samesite")).toBe("Strict");
    expect(cookie.attributes.has("secure") && cookie.attributes.has("httponly")).toBe(true);
  });
});

describe("createSession", () => {
  it("stores only SHA-256(token) with 7 day idle and 30 day absolute lifetimes", async () => {
    const user = await createTestUser(env);
    const t = 1_900_000_000;
    const created = await createSession(env, { userId: user.id, amr: "passkey", now: t, reauthAt: t, userAgent: "Agent/1.0" });
    expect(created.token).toMatch(/^[A-Za-z0-9_-]{43}$/);
    expect(created.idHash).toBe(await sha256Hex(created.token));
    expect(created.maxAge).toBe(ABSOLUTE_SECONDS);
    await env.DB.batch([created.statement]);
    expect(await sessionRow(created.idHash)).toMatchObject({
      user_id: user.id, created_at: t, last_seen_at: t, idle_expires_at: t + IDLE_SECONDS, expires_at: t + ABSOLUTE_SECONDS,
      reauth_at: t, amr: "passkey", recovery_only: 0, user_agent: "Agent/1.0", revoked_at: null,
    });
    const rows = await env.DB.prepare("SELECT count(*) AS n FROM sessions WHERE id_hash = ? OR id_hash = ?").bind(created.token, created.idHash).first<{ n: number }>();
    expect(rows?.n).toBe(1);
  });

  it("gives a recovery_only session 30 minutes in total and truncates the user agent", async () => {
    const user = await createTestUser(env);
    const created = await createSession(env, { userId: user.id, amr: "recovery", recoveryOnly: true, userAgent: "x".repeat(500), now: 1_900_000_000 });
    await created.statement.run();
    const row = await sessionRow(created.idHash);
    expect(row).toMatchObject({ recovery_only: 1, amr: "recovery", expires_at: 1_900_000_000 + RECOVERY_ONLY_SECONDS, idle_expires_at: 1_900_000_000 + RECOVERY_ONLY_SECONDS, reauth_at: null });
    expect((row?.user_agent as string).length).toBe(200);
    expect(created.maxAge).toBe(1800);
  });

  it("returns a statement the caller can put in its own batch", async () => {
    const user = await createTestUser(env);
    const created = await createSession(env, { userId: user.id, amr: "passkey" });
    expect(await sessionRow(created.idHash)).toBeNull();
    await env.DB.batch([env.DB.prepare("UPDATE users SET display_name = 'Batch' WHERE id = ?").bind(user.id), created.statement]);
    expect(await sessionRow(created.idHash)).not.toBeNull();
  });
});

describe("auth resolver: session", () => {
  it("answers 401 without a cookie (no Set-Cookie) and 401 with an unknown token (cookie cleared)", async () => {
    const none = await call("/api/me");
    await expectError(none, 401, "unauthorized");
    expect(setCookies(none)).toEqual([]);

    const unknown = await call("/api/me", { headers: { Cookie: `${SESSION_COOKIE}=not-a-session` } });
    await expectError(unknown, 401, "unauthorized");
    expect(parseSetCookie(setCookies(unknown)[0] ?? "").attributes.get("max-age")).toBe("0");
  });

  it("ignores other cookies and never reads the cookie on a public route", async () => {
    const res = await call("/api/me", { headers: { Cookie: "mc_session=abc; other=1" } });
    await expectError(res, 401, "unauthorized");
    const health = await call("/api/health", { headers: { Cookie: `${SESSION_COOKIE}=garbage` } });
    expect(health.status).toBe(200);
    expect(setCookies(health)).toEqual([]);
  });

  it("accepts a valid session and finds it among several cookies", async () => {
    const user = await createTestUser(env);
    const session = await loginAs(user.id);
    const res = await call("/api/me", { headers: { Cookie: `a=1; ${session.cookie}; b=2` } });
    expect(res.status).toBe(200);
  });

  it("rejects an idle-expired session (7 days) and an absolutely expired one (30 days)", async () => {
    const user = await createTestUser(env);
    const idle = await loginAs(user.id);
    const absolute = await loginAs(user.id);
    expect((await authedCall(idle, "/api/me")).status).toBe(200);
    await env.DB.prepare("UPDATE sessions SET idle_expires_at = ? WHERE id_hash = ?").bind(now() - 1, idle.idHash).run();
    await expectError(await authedCall(idle, "/api/me"), 401, "unauthorized");
    await env.DB.prepare("UPDATE sessions SET expires_at = ? WHERE id_hash = ?").bind(now() - 1, absolute.idHash).run();
    await expectError(await authedCall(absolute, "/api/me"), 401, "unauthorized");
  });

  it("rejects a session whose row is marked revoked", async () => {
    const session = await loginAs((await createTestUser(env)).id);
    await env.DB.prepare("UPDATE sessions SET revoked_at = ? WHERE id_hash = ?").bind(now(), session.idHash).run();
    await expectError(await authedCall(session, "/api/me"), 401, "unauthorized");
  });

  it("rejects a session whose user is gone", async () => {
    const user = await createTestUser(env);
    const session = await loginAs(user.id);
    await env.DB.prepare("DELETE FROM users WHERE id = ?").bind(user.id).run();
    await expectError(await authedCall(session, "/api/me"), 401, "unauthorized");
  });

  it("answers 403 blocked for a blocked user", async () => {
    const user = await createTestUser(env, { status: "blocked" });
    const session = await loginAs(user.id);
    await expectError(await authedCall(session, "/api/me"), 403, "blocked");
  });

  it("revoking takes effect on the next request", async () => {
    const session = await loginAs((await createTestUser(env)).id);
    expect((await authedCall(session, "/api/me")).status).toBe(200);
    await env.DB.prepare("DELETE FROM sessions WHERE id_hash = ?").bind(session.idHash).run();
    await expectError(await authedCall(session, "/api/me"), 401, "unauthorized");
  });
});

describe("session touch: at most one write per hour", () => {
  it("writes last_seen_at and the idle expiry only when the last write is an hour old", async () => {
    const user = await createTestUser(env);
    const t = now();
    const session = await loginAs(user.id, { now: t - 7200 });
    const get = () => authedCall(session, "/api/me");

    // Last seen 2 h ago: the first request writes.
    expect((await get()).status).toBe(200);
    let row = await sessionRow(session.idHash);
    expect(row?.last_seen_at as number).toBeGreaterThanOrEqual(t);
    expect(row?.idle_expires_at as number).toBeGreaterThanOrEqual(t + IDLE_SECONDS);

    // Within the hour: no write. A sentinel in idle_expires_at would be overwritten by a touch.
    const sentinel = t + 1234;
    await env.DB.prepare("UPDATE sessions SET idle_expires_at = ? WHERE id_hash = ?").bind(sentinel, session.idHash).run();
    for (let i = 0; i < 3; i += 1) expect((await get()).status).toBe(200);
    row = await sessionRow(session.idHash);
    expect(row?.idle_expires_at).toBe(sentinel);

    // Moved back by more than an hour: the next request writes again.
    await env.DB.prepare("UPDATE sessions SET last_seen_at = ? WHERE id_hash = ?").bind(t - 3601, session.idHash).run();
    expect((await get()).status).toBe(200);
    row = await sessionRow(session.idHash);
    expect(row?.idle_expires_at as number).toBeGreaterThanOrEqual(t + IDLE_SECONDS);
    expect(row?.last_seen_at as number).toBeGreaterThanOrEqual(t);
  });

  it("never extends the idle expiry beyond the absolute expiry", async () => {
    const t = now();
    const session = await loginAs((await createTestUser(env)).id, { now: t - 7200 });
    await env.DB.prepare("UPDATE sessions SET expires_at = ? WHERE id_hash = ?").bind(t + 600, session.idHash).run();
    expect((await authedCall(session, "/api/me")).status).toBe(200);
    expect((await sessionRow(session.idHash))?.idle_expires_at).toBe(t + 600);
  });

  it("does not write on a refused request", async () => {
    const t = now();
    const session = await loginAs((await createTestUser(env)).id, { now: t - 7200 });
    await env.DB.prepare("UPDATE sessions SET idle_expires_at = ? WHERE id_hash = ?").bind(t + 99, session.idHash).run();
    // `fresh` route with a non-fresh session: 403, no touch.
    await expectError(await authedCall(session, "/api/pair/x/approve", { method: "POST", body: "{}" }), 403, "fresh_required");
    expect((await sessionRow(session.idHash))?.idle_expires_at).toBe(t + 99);
  });
});

describe("recovery_only sessions", () => {
  it("may call GET /api/me, POST /api/auth/logout and the add-passkey routes, and nothing else", async () => {
    const session = await loginAs((await createTestUser(env)).id, { recoveryOnly: true });
    const me = await authedCall(session, "/api/me");
    expect(me.status).toBe(200);
    expect(((await me.json()) as { recoveryOnly: boolean }).recoveryOnly).toBe(true);

    // allowRecoveryOnly routes are `fresh` routes: a recovery session satisfies fresh, so the handler runs
    // (options answers 200, verify answers 400 for the empty body).
    expect((await authedCall(session, "/api/passkeys/options", { method: "POST", body: "{}" })).status).toBe(200);
    expect((await authedCall(session, "/api/passkeys/verify", { method: "POST", body: "{}" })).status).toBe(400);
    // Everything else is refused, whatever the auth kind.
    for (const [method, path] of [
      ["GET", "/api/sessions"],
      ["GET", "/api/passkeys"],
      ["PATCH", "/api/me"],
      ["POST", "/api/invites"],
      ["DELETE", "/api/passkeys/x"],
      ["POST", "/api/recovery-codes/regenerate"],
      ["GET", "/api/desktops"],
      ["GET", "/api/admin/users"],
    ] as const) {
      await expectError(await authedCall(session, path, { method, body: method === "GET" || method === "DELETE" ? undefined : "{}" }), 403, "recovery_only");
    }
    expect((await authedCall(session, "/api/auth/logout", { method: "POST" })).status).toBe(200);
    await expectError(await authedCall(session, "/api/me"), 401, "unauthorized");
  });
});

describe("fresh and admin", () => {
  const approve = (cookie: string) => call("/api/pair/x/approve", { method: "POST", headers: { Origin: env.RP_ORIGIN, "X-Monoptah-CSRF": "1", "Content-Type": "application/json", Cookie: cookie }, body: "{}" });

  it("fresh routes need reauth_at within the last 5 minutes", async () => {
    const user = await createTestUser(env);
    const plain = await loginAs(user.id);
    await expectError(await approve(plain.cookie), 403, "fresh_required");
    const fresh = await loginAs(user.id, { fresh: true });
    expect((await approve(fresh.cookie)).status).toBe(501);
    await env.DB.prepare("UPDATE sessions SET reauth_at = ? WHERE id_hash = ?").bind(now() - 299, fresh.idHash).run();
    expect((await approve(fresh.cookie)).status).toBe(501);
    await env.DB.prepare("UPDATE sessions SET reauth_at = ? WHERE id_hash = ?").bind(now() - 301, fresh.idHash).run();
    await expectError(await approve(fresh.cookie), 403, "fresh_required");
  });

  it("admin routes need role admin (403 forbidden) and a fresh session (403 fresh_required)", async () => {
    const user = await createTestUser(env);
    const admin = await createTestUser(env, { role: "admin" });
    await expectError(await authedCall(await loginAs(user.id, { fresh: true }), "/api/admin/users"), 403, "forbidden");
    await expectError(await authedCall(await loginAs(admin.id), "/api/admin/users"), 403, "fresh_required");
    expect((await authedCall(await loginAs(admin.id, { fresh: true }), "/api/admin/users")).status).toBe(200);
  });

  it("session+grant routes resolve the session only (the grant is WS-R)", async () => {
    const session = await loginAs((await createTestUser(env)).id);
    const res = await call("/relay/device/x?device=y", { headers: { Upgrade: "websocket", Origin: env.RP_ORIGIN, Cookie: session.cookie } });
    expect(res.status).toBe(501);
    await expectError(await call("/relay/device/x?device=y", { headers: { Upgrade: "websocket", Origin: env.RP_ORIGIN } }), 401, "unauthorized");
    const recovery = await loginAs((await createTestUser(env)).id, { recoveryOnly: true });
    await expectError(await call("/relay/device/x?device=y", { headers: { Upgrade: "websocket", Origin: env.RP_ORIGIN, Cookie: recovery.cookie } }), 403, "recovery_only");
  });

  it("token routes ignore the session cookie entirely", async () => {
    const session = await loginAs((await createTestUser(env)).id, { fresh: true });
    const res = await call("/api/desktop/me", { headers: { Cookie: session.cookie } });
    expect(res.status).toBe(501);
  });

  it("requireFresh lets a handler demand freshness for part of a route (freshWhen)", async () => {
    const route = findRoute("GET", "/api/me") as ApiRoute;
    const app = new Hono<AppEnv>();
    app.get("/probe", authResolver({ ...route, path: "/probe" }), (c) => requireFresh(c) ?? c.json({ ok: true }));
    const send = (cookie: string) => app.request("/probe", { headers: { Cookie: cookie } }, env);
    const user = await createTestUser(env);
    expect((await send((await loginAs(user.id)).cookie)).status).toBe(403);
    expect((await send((await loginAs(user.id, { fresh: true })).cookie)).status).toBe(200);
    expect(isFresh({ reauthAt: null })).toBe(false);
    expect(isFresh({ reauthAt: now() - 300 })).toBe(true);
    expect(isFresh({ reauthAt: now() - 301 })).toBe(false);
  });
});

describe("general limit: 120 per session per minute (in memory)", () => {
  it("answers 429 with Retry-After after 120 calls, audits the first breach once, spares other sessions", async () => {
    const user = await createTestUser(env);
    const session = await loginAs(user.id);
    const other = await loginAs((await createTestUser(env)).id);
    for (let i = 0; i < SESSION_LIMIT; i += 1) expect((await authedCall(session, "/api/me")).status, `call ${i + 1}`).toBe(200);
    const over = await authedCall(session, "/api/me");
    await expectError(over, 429, "rate_limited");
    expect(Number(over.headers.get("Retry-After"))).toBeGreaterThanOrEqual(1);
    expect(Number(over.headers.get("Retry-After"))).toBeLessThanOrEqual(60);
    await expectError(await authedCall(session, "/api/me"), 429, "rate_limited");
    expect((await authedCall(other, "/api/me")).status).toBe(200);
    const { results } = await env.DB.prepare("SELECT meta FROM audit_log WHERE action = 'ratelimit.breach' AND actor_user_id = ?").bind(user.id).all<{ meta: string }>();
    expect(results).toHaveLength(1);
    expect(JSON.parse(results[0]?.meta ?? "{}")).toMatchObject({ group: "session", limit: 120, windowSeconds: 60 });
  });
});

describe("GET /api/sessions", () => {
  it("lists own live sessions with a current flag and derived ids, never other users or dead sessions", async () => {
    const user = await createTestUser(env);
    const a = await loginAs(user.id, { userAgent: "A-agent", now: now() - 100 });
    const b = await loginAs(user.id, { userAgent: "B".repeat(400) });
    const expired = await loginAs(user.id);
    const revoked = await loginAs(user.id);
    const foreign = await loginAs((await createTestUser(env)).id);
    await env.DB.prepare("UPDATE sessions SET expires_at = ? WHERE id_hash = ?").bind(now() - 5, expired.idHash).run();
    await env.DB.prepare("UPDATE sessions SET revoked_at = ? WHERE id_hash = ?").bind(now(), revoked.idHash).run();

    const res = await authedCall(a, "/api/sessions");
    expect(res.status).toBe(200);
    const body = SessionsResponse.parse(await res.json());
    expect(body.sessions).toHaveLength(2);
    expect(body.sessions.filter((s) => s.current)).toHaveLength(1);
    const ids = body.sessions.map((s) => s.id);
    for (const hash of [a.idHash, b.idHash, expired.idHash, revoked.idHash, foreign.idHash]) expect(ids).not.toContain(hash);
    for (const id of ids) expect(id).toMatch(/^[0-9a-f]{32}$/);
    expect(JSON.stringify(body)).not.toContain(a.token);
    const current = body.sessions.find((s) => s.current);
    expect(current?.userAgent).toBe("A-agent");
    expect(current?.amr).toBe("passkey");
    expect(body.sessions.find((s) => !s.current)?.userAgent).toHaveLength(200);
    // Newest first.
    expect(body.sessions[0]?.createdAt).toBeGreaterThanOrEqual(body.sessions[1]?.createdAt ?? 0);
  });
});

describe("DELETE /api/sessions/:id", () => {
  async function idOf(session: { idHash: string }) {
    return (await sha256Hex(session.idHash)).slice(0, 32);
  }

  it("revokes another own session: the next request with it is 401, and room.revoke runs for each desktop", async () => {
    const user = await createTestUser(env);
    const keep = await loginAs(user.id);
    const victim = await loginAs(user.id);
    const d1 = await createTestDesktop(env, user.id);
    const d2 = await createTestDesktop(env, user.id);
    await createTestDesktop(env, user.id, { revoked: true });
    const room = fakeRoomNamespace();

    const res = await authedCall(keep, `/api/sessions/${await idOf(victim)}`, { method: "DELETE" }, withRoom(env, room));
    expect(res.status).toBe(200);
    expect(await res.json()).toEqual({ ok: true });
    expect(setCookies(res)).toEqual([]);
    await expectError(await authedCall(victim, "/api/me"), 401, "unauthorized");
    expect((await authedCall(keep, "/api/me")).status).toBe(200);

    expect(room.calls.map((c) => c.desktopId).sort()).toEqual([d1, d2].sort());
    for (const c of room.calls) {
      expect(c.target).toEqual({ sessionIdHash: victim.idHash });
      expect(c.code).toBe(4001);
    }
    const audit = await env.DB.prepare("SELECT action, target_id, meta FROM audit_log WHERE actor_user_id = ? AND action = 'session.revoke'").bind(user.id).first<{ target_id: string; meta: string }>();
    expect(audit?.target_id).toBe(await idOf(victim));
    expect(JSON.parse(audit?.meta ?? "{}")).toEqual({ all: false, count: 1 });
  });

  it("deleting the current session clears the cookie", async () => {
    const session = await loginAs((await createTestUser(env)).id);
    const res = await authedCall(session, `/api/sessions/${await idOf(session)}`, { method: "DELETE" });
    expect(res.status).toBe(200);
    expect(parseSetCookie(setCookies(res)[0] ?? "").attributes.get("max-age")).toBe("0");
    await expectError(await authedCall(session, "/api/me"), 401, "unauthorized");
  });

  it("answers 404 for an unknown id and for another user's session, which keeps working", async () => {
    const mine = await loginAs((await createTestUser(env)).id);
    const theirs = await loginAs((await createTestUser(env)).id);
    await expectError(await authedCall(mine, "/api/sessions/nope", { method: "DELETE" }), 404, "not_found");
    await expectError(await authedCall(mine, `/api/sessions/${await idOf(theirs)}`, { method: "DELETE" }), 404, "not_found");
    await expectError(await authedCall(mine, `/api/sessions/${theirs.idHash}`, { method: "DELETE" }), 404, "not_found");
    expect((await authedCall(theirs, "/api/me")).status).toBe(200);
  });
});

describe("POST /api/auth/logout", () => {
  it("deletes the row, closes the sockets of the session and audits it", async () => {
    const user = await createTestUser(env);
    const session = await loginAs(user.id);
    const other = await loginAs(user.id);
    const desktop = await createTestDesktop(env, user.id);
    const room = fakeRoomNamespace();
    const res = await authedCall(session, "/api/auth/logout", { method: "POST" }, withRoom(env, room));
    expect(res.status).toBe(200);
    expect(await sessionRow(session.idHash)).toBeNull();
    expect(await sessionRow(other.idHash)).not.toBeNull();
    expect(room.calls).toEqual([{ desktopId: desktop, target: { sessionIdHash: session.idHash }, code: 4001 }]);
    await expectError(await authedCall(session, "/api/me"), 401, "unauthorized");
    const audit = await env.DB.prepare("SELECT meta FROM audit_log WHERE actor_user_id = ? AND action = 'session.logout'").bind(user.id).first<{ meta: string }>();
    expect(audit?.meta).toBe("{}");
  });

  it("needs the browser headers like every cookie POST", async () => {
    const session = await loginAs((await createTestUser(env)).id);
    await expectError(await call("/api/auth/logout", { method: "POST", headers: { Cookie: session.cookie } }), 403, "bad_origin");
    expect(await sessionRow(session.idHash)).not.toBeNull();
  });
});

describe("revoke helpers (sign out everywhere, recovery, block)", () => {
  it("revokeAllSessions deletes every session of the user but the excepted one, and only that user's", async () => {
    const user = await createTestUser(env);
    const [s1, s2, keep] = [await loginAs(user.id), await loginAs(user.id), await loginAs(user.id)];
    const foreign = await loginAs((await createTestUser(env)).id);
    const desktop = await createTestDesktop(env, user.id);
    const room = fakeRoomNamespace();
    const count = await revokeAllSessions(withRoom(env, room), user.id, keep.idHash);
    expect(count).toBe(2);
    expect(await sessionRow(s1.idHash)).toBeNull();
    expect(await sessionRow(s2.idHash)).toBeNull();
    expect(await sessionRow(keep.idHash)).not.toBeNull();
    expect(await sessionRow(foreign.idHash)).not.toBeNull();
    expect(room.calls.map((c) => (c.target as { sessionIdHash: string }).sessionIdHash).sort()).toEqual([s1.idHash, s2.idHash].sort());
    expect(new Set(room.calls.map((c) => c.desktopId))).toEqual(new Set([desktop]));
  });

  it("the block case: sessions are deleted by the helper and the next request is 401", async () => {
    const user = await createTestUser(env);
    const sessions = [await loginAs(user.id), await loginAs(user.id, { fresh: true })];
    for (const s of sessions) expect((await authedCall(s, "/api/me")).status).toBe(200);
    await env.DB.prepare("UPDATE users SET status = 'blocked', blocked_at = ? WHERE id = ?").bind(now(), user.id).run();
    await expectError(await authedCall(sessions[0] as { cookie: string }, "/api/me"), 403, "blocked");
    // The admin block (A-09) batches the statement and closes sockets afterwards:
    const hashes = await listSessionIdHashes(env, user.id);
    expect(hashes.sort()).toEqual(sessions.map((s) => s.idHash).sort());
    await env.DB.batch([deleteAllSessionsStatement(env, user.id)]);
    const room = fakeRoomNamespace();
    await createTestDesktop(env, user.id);
    await closeSessionSockets(withRoom(env, room), user.id, hashes);
    expect(room.calls).toHaveLength(2);
    for (const s of sessions) {
      expect(await sessionRow(s.idHash)).toBeNull();
      await expectError(await authedCall(s, "/api/me"), 401, "unauthorized");
    }
  });

  it("revokeSessions with a hash of another user deletes nothing", async () => {
    const user = await createTestUser(env);
    const foreign = await loginAs((await createTestUser(env)).id);
    expect(await revokeSessions(env, user.id, [foreign.idHash])).toBe(0);
    expect(await sessionRow(foreign.idHash)).not.toBeNull();
  });

  it("calls room.revoke for the user's desktops at most MAX_REVOKE_CALLS times and survives a failing room", async () => {
    const user = await createTestUser(env);
    for (let i = 0; i < 5; i += 1) await createTestDesktop(env, user.id);
    const hashes = Array.from({ length: 30 }, (_, i) => `hash-${i}`);
    const room = fakeRoomNamespace();
    await closeSessionSockets(withRoom(env, room), user.id, hashes);
    expect(room.calls).toHaveLength(MAX_REVOKE_CALLS);

    const failing = withRoom(env, { namespace: { idFromName: (n: string) => ({ n }), get: () => ({ revoke: () => Promise.reject(new Error("boom")) }) } as unknown as typeof env.ROOM });
    await expect(closeSessionSockets(failing, user.id, ["h"])).resolves.toBeUndefined();
  });

  it("makes no room call when the user has no desktop or no session was revoked", async () => {
    const user = await createTestUser(env);
    const room = fakeRoomNamespace();
    await closeSessionSockets(withRoom(env, room), user.id, ["h"]);
    expect(room.calls).toEqual([]);
    await createTestDesktop(env, user.id);
    await closeSessionSockets(withRoom(env, room), user.id, []);
    expect(room.calls).toEqual([]);
  });
});
