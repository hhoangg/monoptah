// R-04: devices (browser profiles).
import { describe, expect, it } from "vitest";
import { authedCall } from "../support/accounts.ts";
import { auditActions, env, fakeRoom, json, newAccount, pairDesktop, randomKey, registerDevice, reportState, requestGrant, sha256HexBytes, withEnv } from "./support/api-support.ts";

describe("POST /api/devices", () => {
  it("registers a device with the fingerprint of the raw key bytes and audits it", async () => {
    const mine = await newAccount();
    const key = randomKey();
    const res = await authedCall(mine.session, "/api/devices", json({ name: "  My phone ", publicKey: key.b64 }));
    expect(res.status).toBe(201);
    const { deviceId } = (await res.json()) as { deviceId: string };
    expect(await env.DB.prepare("SELECT user_id, name, public_key, fingerprint, revoked_at FROM devices WHERE id = ?").bind(deviceId).first()).toEqual({
      user_id: mine.user.id,
      name: "My phone",
      public_key: key.b64,
      fingerprint: await sha256HexBytes(key.bytes),
      revoked_at: null,
    });
    expect(await auditActions(mine.user.id)).toEqual(["device.create"]);
  });

  it("is idempotent for the same key: it returns the same id with no second row or audit event", async () => {
    const mine = await newAccount();
    const first = await registerDevice(mine, "One");
    const again = await authedCall(mine.session, "/api/devices", json({ name: "Renamed", publicKey: first.publicKey }));
    expect(again.status).toBe(200);
    expect(await again.json()).toEqual({ deviceId: first.deviceId });
    expect(await env.DB.prepare("SELECT COUNT(*) AS n FROM devices WHERE user_id = ?").bind(mine.user.id).first()).toEqual({ n: 1 });
    expect(await auditActions(mine.user.id)).toEqual(["device.create"]);
  });

  it("gives two users with the same key two devices, and a forgotten key a new device id", async () => {
    const a = await newAccount();
    const b = await newAccount();
    const key = randomKey();
    const idA = ((await (await authedCall(a.session, "/api/devices", json({ name: "x", publicKey: key.b64 }))).json()) as { deviceId: string }).deviceId;
    const idB = ((await (await authedCall(b.session, "/api/devices", json({ name: "x", publicKey: key.b64 }))).json()) as { deviceId: string }).deviceId;
    expect(idA).not.toBe(idB);
    await authedCall(a.session, `/api/devices/${idA}`, { method: "DELETE" });
    const idA2 = ((await (await authedCall(a.session, "/api/devices", json({ name: "x", publicKey: key.b64 }))).json()) as { deviceId: string }).deviceId;
    expect(idA2).not.toBe(idA);
  });

  it("answers quota_exceeded at the 11th device and counts only live ones", async () => {
    const mine = await newAccount();
    const ids: string[] = [];
    for (let i = 0; i < 10; i += 1) ids.push((await registerDevice(mine, `D${i}`)).deviceId);
    const res = await authedCall(mine.session, "/api/devices", json({ name: "eleventh", publicKey: randomKey().b64 }));
    expect(res.status).toBe(409);
    expect(await res.json()).toMatchObject({ code: "quota_exceeded" });
    expect(await env.DB.prepare("SELECT COUNT(*) AS n FROM devices WHERE user_id = ?").bind(mine.user.id).first()).toEqual({ n: 10 });
    await authedCall(mine.session, `/api/devices/${ids[0]}`, { method: "DELETE" });
    expect((await authedCall(mine.session, "/api/devices", json({ name: "again", publicKey: randomKey().b64 }))).status).toBe(201);
  });

  it("honours MAX_DEVICES_PER_USER: 2 allows two, a bad value falls back to 10", async () => {
    const two = withEnv({ MAX_DEVICES_PER_USER: "2" });
    const mine = await newAccount();
    await registerDevice(mine, "a", two);
    await registerDevice(mine, "b", two);
    const res = await authedCall(mine.session, "/api/devices", json({ name: "c", publicKey: randomKey().b64 }), two);
    expect(res.status).toBe(409);
    const bad = withEnv({ MAX_DEVICES_PER_USER: "0" });
    const other = await newAccount();
    for (let i = 0; i < 10; i += 1) await registerDevice(other, `D${i}`, bad);
    expect((await authedCall(other.session, "/api/devices", json({ name: "x", publicKey: randomKey().b64 }), bad)).status).toBe(409);
  });

  it("does not let parallel registrations pass the quota together", async () => {
    const one = withEnv({ MAX_DEVICES_PER_USER: "1" });
    const mine = await newAccount();
    const results = await Promise.all([1, 2, 3].map(() => authedCall(mine.session, "/api/devices", json({ name: "p", publicKey: randomKey().b64 }), one)));
    expect(results.map((r) => r.status).sort()).toEqual([201, 409, 409]);
  });

  it("refuses a bad name or a key that is not 32 bytes of base64url with 400", async () => {
    const mine = await newAccount();
    const good = randomKey().b64;
    for (const body of [{ name: "", publicKey: good }, { name: "x", publicKey: btoa("short") }, { name: "x", publicKey: `${good}=` }, { name: "x" }, { name: "x".repeat(65), publicKey: good }]) {
      expect((await authedCall(mine.session, "/api/devices", json(body))).status).toBe(400);
    }
  });
});

describe("GET, PATCH /api/devices", () => {
  it("lists only the caller's live devices and renames with 404 for others'", async () => {
    const mine = await newAccount();
    const theirs = await newAccount();
    const a = await registerDevice(mine, "Alpha");
    const gone = await registerDevice(mine, "Gone");
    const other = await registerDevice(theirs, "Theirs");
    await authedCall(mine.session, `/api/devices/${gone.deviceId}`, { method: "DELETE" });
    const list = (await (await authedCall(mine.session, "/api/devices")).json()) as { devices: { id: string; name: string; fingerprint: string; lastSeenAt: number | null }[] };
    expect(list.devices.map((d) => d.name)).toEqual(["Alpha"]);
    expect(list.devices[0]?.fingerprint).toBe(await sha256HexBytes(a.bytes));

    const renamed = await authedCall(mine.session, `/api/devices/${a.deviceId}`, json({ name: " Pixel " }, "PATCH"));
    expect(((await renamed.json()) as { device: { name: string } }).device.name).toBe("Pixel");
    expect((await authedCall(mine.session, `/api/devices/${other.deviceId}`, json({ name: "hijack" }, "PATCH"))).status).toBe(404);
    expect((await authedCall(mine.session, `/api/devices/${gone.deviceId}`, json({ name: "x" }, "PATCH"))).status).toBe(404);
    expect((await authedCall(mine.session, `/api/devices/${a.deviceId}`, json({ name: "" }, "PATCH"))).status).toBe(400);
    expect(await env.DB.prepare("SELECT name FROM devices WHERE id = ?").bind(other.deviceId).first()).toEqual({ name: "Theirs" });
  });
});

describe("DELETE /api/devices/:id (forget)", () => {
  it("revokes every grant, deletes its push subscriptions, audits it, and tells each desktop's room (4003 and grants)", async () => {
    const room = fakeRoom();
    const roomEnv = withEnv({ ROOM: room.namespace });
    const mine = await newAccount();
    const d1 = await pairDesktop(mine, { testEnv: roomEnv });
    const d2 = await pairDesktop(mine, { testEnv: roomEnv });
    const d3 = await pairDesktop(mine, { testEnv: roomEnv });
    const device = await registerDevice(mine, "Phone", roomEnv);
    const keep = await registerDevice(mine, "Keep", roomEnv);
    await requestGrant(mine, d1.desktopId, device.deviceId, roomEnv);
    await requestGrant(mine, d2.desktopId, device.deviceId, roomEnv);
    await reportState(d2.token, device.deviceId, { state: "confirmed", permission: "read" }, roomEnv);
    await requestGrant(mine, d3.desktopId, keep.deviceId, roomEnv);
    for (const [id, deviceId] of [["s1", device.deviceId], ["s2", keep.deviceId]] as const) {
      await env.DB.prepare("INSERT INTO push_subscriptions (id, user_id, device_id, endpoint, p256dh, auth, vapid_key_id, created_at) VALUES (?, ?, ?, ?, 'p', 'a', 'k', 1)").bind(id, mine.user.id, deviceId, `https://push.example/${id}`).run();
    }
    room.calls.revokes.length = 0;
    room.calls.notifies.length = 0;

    const res = await authedCall(mine.session, `/api/devices/${device.deviceId}`, { method: "DELETE" }, roomEnv);
    expect(res.status).toBe(200);
    expect((await env.DB.prepare("SELECT revoked_at FROM devices WHERE id = ?").bind(device.deviceId).first<{ revoked_at: number }>())?.revoked_at).toBeGreaterThan(0);
    expect(await env.DB.prepare("SELECT COUNT(*) AS n FROM device_grants WHERE device_id = ? AND state != 'revoked'").bind(device.deviceId).first()).toEqual({ n: 0 });
    expect(await env.DB.prepare("SELECT id FROM push_subscriptions").all()).toMatchObject({ results: [{ id: "s2" }] });
    expect(await env.DB.prepare("SELECT state FROM device_grants WHERE device_id = ?").bind(keep.deviceId).first()).toEqual({ state: "pending" });
    expect(room.calls.revokes.map((r) => r.desktopId).sort()).toEqual([d1.desktopId, d2.desktopId].sort());
    expect(room.calls.revokes.every((r) => JSON.stringify(r.target) === JSON.stringify({ deviceId: device.deviceId }) && r.code === 4003)).toBe(true);
    expect(room.calls.notifies.sort()).toEqual([d1.desktopId, d2.desktopId].sort());
    const audit = await env.DB.prepare("SELECT meta FROM audit_log WHERE action = 'device.revoke' AND target_id = ?").bind(device.deviceId).first<{ meta: string }>();
    expect(JSON.parse(audit?.meta ?? "{}")).toEqual({ deviceId: device.deviceId, grants: 2 });
    // The desktop can no longer resurrect the revoked grant.
    expect((await reportState(d2.token, device.deviceId, { state: "confirmed", permission: "control" }, roomEnv)).status).toBe(409);
  });

  it("answers 404 for another user's device, an unknown or an already forgotten one, with no room call", async () => {
    const room = fakeRoom();
    const roomEnv = withEnv({ ROOM: room.namespace });
    const mine = await newAccount();
    const theirs = await newAccount();
    const other = await registerDevice(theirs);
    const mineDevice = await registerDevice(mine);
    expect((await authedCall(mine.session, `/api/devices/${other.deviceId}`, { method: "DELETE" }, roomEnv)).status).toBe(404);
    expect((await authedCall(mine.session, `/api/devices/${crypto.randomUUID()}`, { method: "DELETE" }, roomEnv)).status).toBe(404);
    await authedCall(mine.session, `/api/devices/${mineDevice.deviceId}`, { method: "DELETE" }, roomEnv);
    expect((await authedCall(mine.session, `/api/devices/${mineDevice.deviceId}`, { method: "DELETE" }, roomEnv)).status).toBe(404);
    expect(room.calls.revokes).toEqual([]);
    expect(await env.DB.prepare("SELECT revoked_at FROM devices WHERE id = ?").bind(other.deviceId).first()).toEqual({ revoked_at: null });
    expect((await auditActions(mine.user.id)).filter((a) => a === "device.revoke")).toHaveLength(1);
  });
});
