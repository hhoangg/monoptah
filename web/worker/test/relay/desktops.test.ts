// R-03 (desktops directory) and R-14a (Unpair).
import { describe, expect, it } from "vitest";
import { sha256Hex } from "../../src/lib/crypto.ts";
import { authedCall, loginAs } from "../support/accounts.ts";
import { auditActions, env, fakeRoom, json, newAccount, pairDesktop, registerDevice, requestGrant, tokenCall, withEnv } from "./support/api-support.ts";

describe("GET /api/desktops", () => {
  it("lists only the caller's live desktops with online from the room and the pending count", async () => {
    const mine = await newAccount();
    const theirs = await newAccount();
    const a = await pairDesktop(mine, { machineName: "Alpha" });
    const b = await pairDesktop(mine, { machineName: "Beta" });
    await pairDesktop(theirs, { machineName: "Not mine" });
    const gone = await pairDesktop(mine, { machineName: "Gone" });
    await env.DB.prepare("UPDATE desktops SET revoked_at = 1 WHERE id = ?").bind(gone.desktopId).run();

    // Two pending grants on Alpha (one from a forgotten device, which does not count), none on Beta.
    const d1 = await registerDevice(mine, "One");
    const d2 = await registerDevice(mine, "Two");
    const d3 = await registerDevice(mine, "Three");
    for (const d of [d1, d2, d3]) await requestGrant(mine, a.desktopId, d.deviceId);
    await env.DB.prepare("UPDATE devices SET revoked_at = 1 WHERE id = ?").bind(d3.deviceId).run();
    await env.DB.prepare("UPDATE device_grants SET state = 'confirmed' WHERE device_id = ?").bind(d2.deviceId).run();

    const room = fakeRoom([b.desktopId]);
    const res = await authedCall(mine.session, "/api/desktops", {}, withEnv({ ROOM: room.namespace }));
    expect(res.status).toBe(200);
    const { desktops } = (await res.json()) as { desktops: { id: string; name: string; online: boolean; pendingCount: number; os: string | null; appVersion: string | null; color: string | null }[] };
    expect(desktops.map((d) => d.name).sort()).toEqual(["Alpha", "Beta"]);
    expect(desktops.find((d) => d.id === a.desktopId)).toMatchObject({ online: false, pendingCount: 1, os: "macos", appVersion: "1.0.0", color: null });
    expect(desktops.find((d) => d.id === b.desktopId)).toMatchObject({ online: true, pendingCount: 0 });
  });

  it("counts a failing room as offline and does not write", async () => {
    const mine = await newAccount();
    await pairDesktop(mine);
    const broken = withEnv({ ROOM: { idFromName: (n: string) => n, get: () => ({ presence: () => Promise.reject(new Error("down")) }) } });
    const before = await env.DB.prepare("SELECT COUNT(*) AS n FROM audit_log").first<{ n: number }>();
    const res = await authedCall(mine.session, "/api/desktops", {}, broken);
    expect(((await res.json()) as { desktops: { online: boolean }[] }).desktops.map((d) => d.online)).toEqual([false]);
    expect(await env.DB.prepare("SELECT COUNT(*) AS n FROM audit_log").first()).toEqual(before);
  });

  it("requires a session", async () => {
    expect((await authedCall({ cookie: "" }, "/api/desktops")).status).toBe(401);
  });
});

describe("GET, PATCH /api/desktops/:id", () => {
  it("returns the directory entry with the public key and the full fingerprint", async () => {
    const mine = await newAccount();
    const paired = await pairDesktop(mine);
    const res = await authedCall(mine.session, `/api/desktops/${paired.desktopId}`);
    expect(res.status).toBe(200);
    const body = (await res.json()) as Record<string, unknown>;
    expect(body).toMatchObject({ id: paired.desktopId, publicKey: paired.pairing.publicKey, name: "Work Mac" });
    expect(body.fingerprint).toMatch(/^[0-9a-f]{64}$/);
  });

  it("answers 404 for another user's desktop, a revoked one and an unknown id, on every method", async () => {
    const mine = await newAccount();
    const theirs = await newAccount();
    const other = await pairDesktop(theirs);
    const revoked = await pairDesktop(mine);
    await env.DB.prepare("UPDATE desktops SET revoked_at = 1 WHERE id = ?").bind(revoked.desktopId).run();
    for (const id of [other.desktopId, revoked.desktopId, crypto.randomUUID()]) {
      expect((await authedCall(mine.session, `/api/desktops/${id}`)).status).toBe(404);
      expect((await authedCall(mine.session, `/api/desktops/${id}`, json({ name: "x" }, "PATCH"))).status).toBe(404);
      expect((await authedCall(mine.session, `/api/desktops/${id}`, { method: "DELETE" })).status).toBe(404);
    }
    expect(await env.DB.prepare("SELECT name, revoked_at FROM desktops WHERE id = ?").bind(other.desktopId).first()).toEqual({ name: "Work Mac", revoked_at: null });
  });

  it("renames and recolors, clears the color with null, and keeps the other field when one is sent", async () => {
    const mine = await newAccount();
    const paired = await pairDesktop(mine);
    const path = `/api/desktops/${paired.desktopId}`;
    const first = await authedCall(mine.session, path, json({ name: "  Home  ", color: "teal" }, "PATCH"));
    expect(((await first.json()) as { desktop: Record<string, unknown> }).desktop).toMatchObject({ name: "Home", color: "teal" });
    await authedCall(mine.session, path, json({ name: "Home 2" }, "PATCH"));
    expect(await env.DB.prepare("SELECT name, color FROM desktops WHERE id = ?").bind(paired.desktopId).first()).toEqual({ name: "Home 2", color: "teal" });
    await authedCall(mine.session, path, json({ color: null }, "PATCH"));
    expect(await env.DB.prepare("SELECT name, color FROM desktops WHERE id = ?").bind(paired.desktopId).first()).toEqual({ name: "Home 2", color: null });
    expect((await authedCall(mine.session, path, json({}, "PATCH"))).status).toBe(400);
    expect((await authedCall(mine.session, path, json({ name: "" }, "PATCH"))).status).toBe(400);
  });
});

describe("DELETE /api/desktops/:id (revoke)", () => {
  it("needs a fresh session and changes nothing without it", async () => {
    const mine = await newAccount();
    const paired = await pairDesktop(mine);
    const stale = await loginAs(mine.user.id, { fresh: false });
    const res = await authedCall(stale, `/api/desktops/${paired.desktopId}`, { method: "DELETE" });
    expect(res.status).toBe(403);
    expect(await res.json()).toMatchObject({ code: "fresh_required" });
    expect(await env.DB.prepare("SELECT revoked_at FROM desktops WHERE id = ?").bind(paired.desktopId).first()).toEqual({ revoked_at: null });
  });

  it("revokes the desktop, all its tokens and grants, audits it, and asks the room to close every socket with 4003", async () => {
    const room = fakeRoom();
    const roomEnv = withEnv({ ROOM: room.namespace });
    const mine = await newAccount();
    const paired = await pairDesktop(mine, { testEnv: roomEnv });
    const second = crypto.randomUUID();
    await env.DB.prepare("INSERT INTO desktop_tokens (id, desktop_id, token_hash, scope, created_at) VALUES (?, ?, ?, 'relay', 1)").bind(second, paired.desktopId, await sha256Hex("second-token-for-test")).run();
    const device = await registerDevice(mine);
    await requestGrant(mine, paired.desktopId, device.deviceId);

    const res = await authedCall(mine.session, `/api/desktops/${paired.desktopId}`, { method: "DELETE" }, roomEnv);
    expect(res.status).toBe(200);
    expect(room.calls.revokes).toEqual([{ desktopId: paired.desktopId, target: { all: true }, code: 4003 }]);
    expect((await env.DB.prepare("SELECT revoked_at FROM desktops WHERE id = ?").bind(paired.desktopId).first<{ revoked_at: number }>())?.revoked_at).toBeGreaterThan(0);
    expect(await env.DB.prepare("SELECT COUNT(*) AS n FROM desktop_tokens WHERE desktop_id = ? AND revoked_at IS NULL").bind(paired.desktopId).first()).toEqual({ n: 0 });
    expect(await env.DB.prepare("SELECT COUNT(*) AS n FROM device_grants WHERE desktop_id = ? AND state != 'revoked'").bind(paired.desktopId).first()).toEqual({ n: 0 });
    expect((await tokenCall("/api/desktop/me", paired.token)).status).toBe(403);
    expect((await auditActions(mine.user.id)).filter((a) => a === "desktop.revoke")).toEqual(["desktop.revoke"]);
    // Gone from the list, and a second delete is 404 without a second audit row or room call.
    expect(((await (await authedCall(mine.session, "/api/desktops")).json()) as { desktops: unknown[] }).desktops).toEqual([]);
    expect((await authedCall(mine.session, `/api/desktops/${paired.desktopId}`, { method: "DELETE" }, roomEnv)).status).toBe(404);
    expect(room.calls.revokes).toHaveLength(1);
    expect((await auditActions(mine.user.id)).filter((a) => a === "desktop.revoke")).toHaveLength(1);
  });

  it("frees a quota slot", async () => {
    const mine = await newAccount();
    const ids: string[] = [];
    for (let i = 0; i < 5; i += 1) ids.push((await pairDesktop(mine)).desktopId);
    await authedCall(mine.session, `/api/desktops/${ids[0]}`, { method: "DELETE" });
    await pairDesktop(mine);
  });
});

describe("Unpair: DELETE /api/desktop/me (R-14a)", () => {
  it("revokes the token (the next call is 403), keeps the desktop row, audits as a desktop and closes the room with 4003", async () => {
    const room = fakeRoom();
    const roomEnv = withEnv({ ROOM: room.namespace });
    const mine = await newAccount();
    const paired = await pairDesktop(mine, { testEnv: roomEnv });
    const res = await tokenCall("/api/desktop/me", paired.token, { method: "DELETE" }, roomEnv);
    expect(res.status).toBe(200);
    expect((await tokenCall("/api/desktop/me", paired.token, {}, roomEnv)).status).toBe(403);
    expect(room.calls.revokes).toEqual([{ desktopId: paired.desktopId, target: { all: true }, code: 4003 }]);
    expect(await env.DB.prepare("SELECT revoked_at FROM desktops WHERE id = ?").bind(paired.desktopId).first()).toEqual({ revoked_at: null });
    const audit = await env.DB.prepare("SELECT actor_kind, actor_user_id, action FROM audit_log WHERE action = 'desktop.revoke' AND target_id = ?").bind(paired.desktopId).first();
    expect(audit).toEqual({ actor_kind: "desktop", actor_user_id: mine.user.id, action: "desktop.revoke" });
  });

  it("lets the unpaired desktop be replaced by a new pairing without an orphan", async () => {
    const mine = await newAccount();
    const paired = await pairDesktop(mine);
    await tokenCall("/api/desktop/me", paired.token, { method: "DELETE" });
    const again = await pairDesktop(mine, { replaces: paired.desktopId });
    expect(again.desktopId).toBe(paired.desktopId);
    expect(await env.DB.prepare("SELECT COUNT(*) AS n FROM desktops WHERE user_id = ?").bind(mine.user.id).first()).toEqual({ n: 1 });
    expect((await tokenCall("/api/desktop/me", again.token)).status).toBe(200);
  });

  it("does not touch another desktop's tokens", async () => {
    const mine = await newAccount();
    const a = await pairDesktop(mine);
    const b = await pairDesktop(mine);
    await tokenCall("/api/desktop/me", a.token, { method: "DELETE" });
    expect((await tokenCall("/api/desktop/me", b.token)).status).toBe(200);
  });
});
