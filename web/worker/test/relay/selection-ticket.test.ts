// Selection tickets: a Control web device proves a fresh passkey re-auth to its desktop with a single-use ticket
// (security.md decision "Selection widening from the web"). Both routes, the rate limit, the audit rows and the cleanup.
import { describe, expect, it } from "vitest";
import { cleanup as grantsCleanup } from "../../src/modules/grants/cleanup.ts";
import { sha256Hex } from "../../src/lib/crypto.ts";
import { authedCall, loginAs } from "../support/accounts.ts";
import { env, fakeRoom, json, newAccount, pairDesktop, registerDevice, reportState, requestGrant, tokenCall, withEnv } from "./support/api-support.ts";

async function setup(permission: "read" | "control" | null = "control") {
  const roomEnv = withEnv({ ROOM: fakeRoom().namespace });
  const mine = await newAccount();
  const desktop = await pairDesktop(mine, { testEnv: roomEnv });
  const device = await registerDevice(mine, "Browser", roomEnv);
  await requestGrant(mine, desktop.desktopId, device.deviceId, roomEnv);
  if (permission) await reportState(desktop.token, device.deviceId, { state: "confirmed", permission }, roomEnv);
  return { roomEnv, mine, desktop, device };
}

const ticketPath = (desktopId: string, deviceId: string) => `/api/desktops/${desktopId}/grants/${deviceId}/selection-ticket`;
const verifyPath = (deviceId: string) => `/api/desktop/grants/${deviceId}/selection-ticket`;

async function mint(s: Awaited<ReturnType<typeof setup>>): Promise<{ ticket: string; expiresAt: number }> {
  const res = await authedCall(s.mine.session, ticketPath(s.desktop.desktopId, s.device.deviceId), json({}), s.roomEnv);
  expect(res.status).toBe(200);
  return (await res.json()) as { ticket: string; expiresAt: number };
}

const verify = (s: Awaited<ReturnType<typeof setup>>, ticket: string, deviceId = s.device.deviceId, token = s.desktop.token) =>
  tokenCall(verifyPath(deviceId), token, json({ ticket }), s.roomEnv);

describe("POST /api/desktops/:id/grants/:deviceId/selection-ticket", () => {
  it("mints a ticket for a Control device, stores only its SHA-256, and audits it", async () => {
    const s = await setup();
    const { ticket, expiresAt } = await mint(s);
    expect(ticket.length).toBeGreaterThanOrEqual(40);
    expect(expiresAt - Math.floor(Date.now() / 1000)).toBeGreaterThan(100);
    expect(expiresAt - Math.floor(Date.now() / 1000)).toBeLessThanOrEqual(120);
    const rows = (await env.DB.prepare("SELECT * FROM selection_tickets WHERE desktop_id = ?").bind(s.desktop.desktopId).all<Record<string, unknown>>()).results;
    expect(rows).toHaveLength(1);
    expect(rows[0]).toMatchObject({ token_hash: await sha256Hex(ticket), user_id: s.mine.user.id, device_id: s.device.deviceId, used_at: null });
    expect(JSON.stringify(rows)).not.toContain(ticket);
    const audit = await env.DB.prepare("SELECT action, meta FROM audit_log WHERE actor_user_id = ? AND action = 'selection.ticket'").bind(s.mine.user.id).all<{ action: string; meta: string }>();
    expect(audit.results).toHaveLength(1);
    expect(audit.results[0]?.meta).not.toContain(ticket);
  });

  it("is 403 fresh_required for a session without a recent passkey re-auth, and mints nothing", async () => {
    const s = await setup();
    const stale = await loginAs(s.mine.user.id, { fresh: false });
    const res = await authedCall(stale, ticketPath(s.desktop.desktopId, s.device.deviceId), json({}), s.roomEnv);
    expect(res.status).toBe(403);
    expect(((await res.json()) as { code: string }).code).toBe("fresh_required");
    expect((await env.DB.prepare("SELECT count(*) AS n FROM selection_tickets WHERE desktop_id = ?").bind(s.desktop.desktopId).first<{ n: number }>())?.n).toBe(0);
  });

  it("needs a re-auth within the last minute, not just the 5 minute fresh window", async () => {
    const s = await setup();
    const now = Math.floor(Date.now() / 1000);
    const at = async (age: number) => {
      const session = await loginAs(s.mine.user.id, { fresh: true, now: now - age });
      return authedCall(session, ticketPath(s.desktop.desktopId, s.device.deviceId), json({}), s.roomEnv);
    };
    expect((await at(30)).status).toBe(200);
    expect((await at(55)).status).toBe(200);
    const old = await at(90);
    expect(old.status).toBe(403);
    expect(((await old.json()) as { code: string }).code).toBe("fresh_required");
    // Still inside the general 5 minute window, but too old to mint.
    expect((await at(250)).status).toBe(403);
    expect((await env.DB.prepare("SELECT count(*) AS n FROM selection_tickets WHERE desktop_id = ?").bind(s.desktop.desktopId).first<{ n: number }>())?.n).toBe(2);
  });

  it("refuses a read grant, a pending grant and a revoked grant with 403 forbidden", async () => {
    for (const permission of ["read", null] as const) {
      const s = await setup(permission);
      const res = await authedCall(s.mine.session, ticketPath(s.desktop.desktopId, s.device.deviceId), json({}), s.roomEnv);
      expect(res.status, String(permission)).toBe(403);
      expect(((await res.json()) as { code: string }).code).toBe("forbidden");
    }
    const s = await setup();
    await reportState(s.desktop.token, s.device.deviceId, { state: "revoked" }, s.roomEnv);
    expect((await authedCall(s.mine.session, ticketPath(s.desktop.desktopId, s.device.deviceId), json({}), s.roomEnv)).status).toBe(403);
    const forgotten = await setup();
    await authedCall(forgotten.mine.session, `/api/devices/${forgotten.device.deviceId}`, { method: "DELETE" }, forgotten.roomEnv);
    expect((await authedCall(forgotten.mine.session, ticketPath(forgotten.desktop.desktopId, forgotten.device.deviceId), json({}), forgotten.roomEnv)).status).toBe(403);
  });

  it("refuses another user's desktop (404) and another user's device (403)", async () => {
    const s = await setup();
    const other = await newAccount();
    expect((await authedCall(other.session, ticketPath(s.desktop.desktopId, s.device.deviceId), json({}), s.roomEnv)).status).toBe(404);
    // The caller's own desktop but somebody else's device.
    const otherDesktop = await pairDesktop(other, { testEnv: s.roomEnv });
    expect((await authedCall(other.session, ticketPath(otherDesktop.desktopId, s.device.deviceId), json({}), s.roomEnv)).status).toBe(403);
    expect((await env.DB.prepare("SELECT count(*) AS n FROM selection_tickets WHERE user_id IN (?, ?)").bind(s.mine.user.id, other.user.id).first<{ n: number }>())?.n).toBe(0);
  });

  it("allows 10 tickets per user per hour, then 429 with Retry-After, and audits the breach once", async () => {
    const s = await setup();
    for (let i = 0; i < 10; i++) await mint(s);
    const res = await authedCall(s.mine.session, ticketPath(s.desktop.desktopId, s.device.deviceId), json({}), s.roomEnv);
    expect(res.status).toBe(429);
    expect(res.headers.get("Retry-After")).not.toBeNull();
    expect(((await res.json()) as { code: string }).code).toBe("rate_limited");
    await authedCall(s.mine.session, ticketPath(s.desktop.desktopId, s.device.deviceId), json({}), s.roomEnv);
    const breaches = await env.DB.prepare("SELECT 1 FROM audit_log WHERE actor_user_id = ? AND action = 'ratelimit.breach'").bind(s.mine.user.id).all();
    expect(breaches.results).toHaveLength(1);
    expect((await env.DB.prepare("SELECT count(*) AS n FROM selection_tickets WHERE desktop_id = ?").bind(s.desktop.desktopId).first<{ n: number }>())?.n).toBe(10);
  });
});

describe("POST /api/desktop/grants/:deviceId/selection-ticket", () => {
  it("accepts a ticket once: 200, then 404 for the same ticket", async () => {
    const s = await setup();
    const { ticket } = await mint(s);
    expect((await verify(s, ticket)).status).toBe(200);
    expect((await env.DB.prepare("SELECT used_at FROM selection_tickets WHERE desktop_id = ?").bind(s.desktop.desktopId).first<{ used_at: number }>())?.used_at).toBeGreaterThan(0);
    const again = await verify(s, ticket);
    expect(again.status).toBe(404);
    expect(((await again.json()) as { code: string }).code).toBe("not_found");
    const widened = await env.DB.prepare("SELECT 1 FROM audit_log WHERE action = 'selection.widen' AND actor_kind = 'desktop' AND actor_user_id = ?").bind(s.mine.user.id).all();
    expect(widened.results).toHaveLength(1);
  });

  it("refuses a ticket once the grant is revoked or lowered to read after it was minted", async () => {
    for (const change of [{ state: "revoked" }, { state: "confirmed", permission: "read" }] as const) {
      const s = await setup();
      const { ticket } = await mint(s);
      await reportState(s.desktop.token, s.device.deviceId, change, s.roomEnv);
      expect((await verify(s, ticket)).status, JSON.stringify(change)).toBe(404);
      expect((await env.DB.prepare("SELECT used_at FROM selection_tickets WHERE desktop_id = ?").bind(s.desktop.desktopId).first<{ used_at: number | null }>())?.used_at).toBeNull();
    }
    const forgotten = await setup();
    const { ticket } = await mint(forgotten);
    await authedCall(forgotten.mine.session, `/api/devices/${forgotten.device.deviceId}`, { method: "DELETE" }, forgotten.roomEnv);
    expect((await verify(forgotten, ticket)).status).toBe(404);
  });

  it("only one of two concurrent verifications succeeds", async () => {
    const s = await setup();
    const { ticket } = await mint(s);
    const statuses = (await Promise.all([verify(s, ticket), verify(s, ticket)])).map((r) => r.status).sort();
    expect(statuses).toEqual([200, 404]);
  });

  it("refuses an expired ticket", async () => {
    const s = await setup();
    const { ticket } = await mint(s);
    await env.DB.prepare("UPDATE selection_tickets SET expires_at = ? WHERE desktop_id = ?").bind(Math.floor(Date.now() / 1000) - 1, s.desktop.desktopId).run();
    expect((await verify(s, ticket)).status).toBe(404);
  });

  it("is bound to the desktop and the device it was made for", async () => {
    const s = await setup();
    const { ticket } = await mint(s);
    // Another desktop of the same user, with the same device.
    const second = await pairDesktop(s.mine, { testEnv: s.roomEnv });
    expect((await verify(s, ticket, s.device.deviceId, second.token)).status).toBe(404);
    // Another device on the right desktop.
    const otherDevice = await registerDevice(s.mine, "Other", s.roomEnv);
    expect((await verify(s, ticket, otherDevice.deviceId)).status).toBe(404);
    // Still valid for the real pair afterwards: the failed tries did not consume it.
    expect((await verify(s, ticket)).status).toBe(200);
  });

  it("refuses an unknown ticket, a bad body and a missing or non-desktop credential", async () => {
    const s = await setup();
    expect((await verify(s, "not-a-ticket")).status).toBe(404);
    expect((await tokenCall(verifyPath(s.device.deviceId), s.desktop.token, json({}), s.roomEnv)).status).toBe(400);
    expect((await tokenCall(verifyPath(s.device.deviceId), null, json({ ticket: "x" }), s.roomEnv)).status).toBe(401);
    const { ticket } = await mint(s);
    const viaSession = await authedCall(s.mine.session, verifyPath(s.device.deviceId), json({ ticket }), s.roomEnv);
    expect(viaSession.status).toBe(401);
  });
});

describe("selection ticket cleanup", () => {
  it("removes tickets that expired more than an hour ago, used or not, and keeps recent ones", async () => {
    const s = await setup();
    const now = Math.floor(Date.now() / 1000);
    const insert = (hash: string, expiresAt: number, usedAt: number | null) =>
      env.DB.prepare("INSERT INTO selection_tickets (token_hash, user_id, desktop_id, device_id, created_at, expires_at, used_at) VALUES (?, ?, ?, ?, 1, ?, ?)").bind(hash, s.mine.user.id, s.desktop.desktopId, s.device.deviceId, expiresAt, usedAt);
    await env.DB.batch([insert("old-unused", now - 7200, null), insert("old-used", now - 7200, now - 7300), insert("recent", now - 60, null), insert("live", now + 100, null)]);
    await grantsCleanup(env, now);
    const left = (await env.DB.prepare("SELECT token_hash FROM selection_tickets WHERE user_id = ? ORDER BY token_hash").bind(s.mine.user.id).all<{ token_hash: string }>()).results.map((r) => r.token_hash);
    expect(left).toEqual(["live", "recent"]);
  });
});
