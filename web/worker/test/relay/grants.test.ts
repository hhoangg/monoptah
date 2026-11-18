// R-04: the web side of the grant state machine (request, list, requested permission, revoke).
import { describe, expect, it } from "vitest";
import { authedCall, loginAs } from "../support/accounts.ts";
import { env, fakeRoom, grantRow, json, newAccount, pairDesktop, registerDevice, reportState, requestGrant, sha256HexBytes, withEnv } from "./support/api-support.ts";

async function setup() {
  const room = fakeRoom();
  const roomEnv = withEnv({ ROOM: room.namespace });
  const mine = await newAccount();
  const desktop = await pairDesktop(mine, { testEnv: roomEnv });
  const device = await registerDevice(mine, "Phone", roomEnv);
  return { room, roomEnv, mine, desktop, device };
}

describe("POST /api/desktops/:id/grants", () => {
  it("creates a pending grant, tells the room to notify the desktop and audits grant.change", async () => {
    const { room, roomEnv, mine, desktop, device } = await setup();
    room.calls.notifies.length = 0;
    const res = await requestGrant(mine, desktop.desktopId, device.deviceId, roomEnv);
    expect(res.status).toBe(201);
    const { grant } = (await res.json()) as { grant: Record<string, unknown> };
    expect(grant).toMatchObject({ deviceId: device.deviceId, deviceName: "Phone", state: "pending", permission: null, requestedPermission: null, confirmedAt: null });
    expect(grant.fingerprint).toBe(await sha256HexBytes(device.bytes));
    expect(room.calls.notifies).toEqual([desktop.desktopId]);
    const meta = await env.DB.prepare("SELECT meta FROM audit_log WHERE action = 'grant.change' AND actor_user_id = ?").bind(mine.user.id).first<{ meta: string }>();
    expect(JSON.parse(meta?.meta ?? "{}")).toEqual({ deviceId: device.deviceId, desktopId: desktop.desktopId, state: "pending" });
  });

  it("returns an existing pending or confirmed grant unchanged, with no second audit event", async () => {
    const { roomEnv, mine, desktop, device } = await setup();
    await requestGrant(mine, desktop.desktopId, device.deviceId, roomEnv);
    const again = await requestGrant(mine, desktop.desktopId, device.deviceId, roomEnv);
    expect(again.status).toBe(200);
    expect(((await again.json()) as { grant: { state: string } }).grant.state).toBe("pending");
    await reportState(desktop.token, device.deviceId, { state: "confirmed", permission: "control" }, roomEnv);
    const confirmed = await requestGrant(mine, desktop.desktopId, device.deviceId, roomEnv);
    expect(((await confirmed.json()) as { grant: Record<string, unknown> }).grant).toMatchObject({ state: "confirmed", permission: "control" });
    expect(await env.DB.prepare("SELECT COUNT(*) AS n FROM audit_log WHERE action = 'grant.change' AND actor_kind = 'user' AND actor_user_id = ?").bind(mine.user.id).first()).toEqual({ n: 1 });
  });

  it("answers 409 for a revoked grant: a revoked device comes back only as a new device", async () => {
    const { roomEnv, mine, desktop, device } = await setup();
    await requestGrant(mine, desktop.desktopId, device.deviceId, roomEnv);
    await authedCall(mine.session, `/api/desktops/${desktop.desktopId}/grants/${device.deviceId}`, json({ state: "revoked" }, "PUT"), roomEnv);
    const res = await requestGrant(mine, desktop.desktopId, device.deviceId, roomEnv);
    expect(res.status).toBe(409);
    expect(await res.json()).toMatchObject({ code: "conflict" });
    const fresh = await registerDevice(mine, "Phone again");
    expect((await requestGrant(mine, desktop.desktopId, fresh.deviceId, roomEnv)).status).toBe(201);
  });

  it("answers 404 for another user's desktop or device, a forgotten device and a revoked desktop", async () => {
    const { roomEnv, mine, desktop, device } = await setup();
    const theirs = await newAccount();
    const theirDesktop = await pairDesktop(theirs);
    const theirDevice = await registerDevice(theirs);
    expect((await requestGrant(mine, theirDesktop.desktopId, device.deviceId, roomEnv)).status).toBe(404);
    expect((await requestGrant(mine, desktop.desktopId, theirDevice.deviceId, roomEnv)).status).toBe(404);
    expect((await requestGrant(theirs, desktop.desktopId, theirDevice.deviceId, roomEnv)).status).toBe(404);
    expect((await requestGrant(mine, desktop.desktopId, crypto.randomUUID(), roomEnv)).status).toBe(404);
    await authedCall(mine.session, `/api/devices/${device.deviceId}`, { method: "DELETE" }, roomEnv);
    expect((await requestGrant(mine, desktop.desktopId, device.deviceId, roomEnv)).status).toBe(404);
    await env.DB.prepare("UPDATE desktops SET revoked_at = 1 WHERE id = ?").bind(desktop.desktopId).run();
    const another = await registerDevice(mine, "Another");
    expect((await requestGrant(mine, desktop.desktopId, another.deviceId, roomEnv)).status).toBe(404);
  });

  it("answers 400 for a body without a deviceId", async () => {
    const { mine, desktop } = await setup();
    expect((await authedCall(mine.session, `/api/desktops/${desktop.desktopId}/grants`, json({}))).status).toBe(400);
  });
});

describe("GET /api/desktops/:id/grants", () => {
  it("lists grants with device name, fingerprint, state, permission, requestedPermission, confirmedAt and updatedAt", async () => {
    const { roomEnv, mine, desktop, device } = await setup();
    const second = await registerDevice(mine, "Tablet");
    await requestGrant(mine, desktop.desktopId, device.deviceId, roomEnv);
    await requestGrant(mine, desktop.desktopId, second.deviceId, roomEnv);
    await reportState(desktop.token, device.deviceId, { state: "confirmed", permission: "read" }, roomEnv);
    const res = await authedCall(mine.session, `/api/desktops/${desktop.desktopId}/grants`);
    const { grants } = (await res.json()) as { grants: Record<string, unknown>[] };
    expect(grants).toHaveLength(2);
    const phone = grants.find((g) => g.deviceId === device.deviceId);
    expect(phone).toMatchObject({ deviceName: "Phone", state: "confirmed", permission: "read", requestedPermission: null });
    expect(phone?.confirmedAt).toBeGreaterThan(0);
    expect(phone?.updatedAt).toBeGreaterThan(0);
    expect(grants.find((g) => g.deviceId === second.deviceId)).toMatchObject({ deviceName: "Tablet", state: "pending", confirmedAt: null });
  });

  it("is 404 for another user's desktop", async () => {
    const { desktop } = await setup();
    const other = await newAccount();
    expect((await authedCall(other.session, `/api/desktops/${desktop.desktopId}/grants`)).status).toBe(404);
  });
});

describe("PUT /api/desktops/:id/grants/:deviceId", () => {
  it("only records requestedPermission: permission and state stay as the desktop set them", async () => {
    const { room, roomEnv, mine, desktop, device } = await setup();
    await requestGrant(mine, desktop.desktopId, device.deviceId, roomEnv);
    await reportState(desktop.token, device.deviceId, { state: "confirmed", permission: "read" }, roomEnv);
    room.calls.notifies.length = 0;
    const res = await authedCall(mine.session, `/api/desktops/${desktop.desktopId}/grants/${device.deviceId}`, json({ requestedPermission: "control" }, "PUT"), roomEnv);
    expect(res.status).toBe(200);
    expect(((await res.json()) as { grant: Record<string, unknown> }).grant).toMatchObject({ state: "confirmed", permission: "read", requestedPermission: "control" });
    expect(room.calls.notifies).toEqual([desktop.desktopId]);
    expect(room.calls.revokes.filter((r) => JSON.stringify(r.target).includes("deviceId"))).toEqual([]);
    const meta = await env.DB.prepare("SELECT meta FROM audit_log WHERE action = 'grant.change' AND actor_kind = 'user' ORDER BY rowid DESC").first<{ meta: string }>();
    expect(JSON.parse(meta?.meta ?? "{}")).toEqual({ deviceId: device.deviceId, desktopId: desktop.desktopId, requestedPermission: "control" });
  });

  it("cannot make a grant confirmed or set a permission, whatever the body carries", async () => {
    const { roomEnv, mine, desktop, device } = await setup();
    await requestGrant(mine, desktop.desktopId, device.deviceId, roomEnv);
    const path = `/api/desktops/${desktop.desktopId}/grants/${device.deviceId}`;
    for (const body of [{ state: "confirmed" }, { permission: "control" }, { state: "pending" }]) {
      const res = await authedCall(mine.session, path, json(body, "PUT"), roomEnv);
      expect(res.status).toBe(400);
    }
    // An unknown key next to a valid one is ignored and never written.
    await authedCall(mine.session, path, json({ requestedPermission: "read", permission: "control" }, "PUT"), roomEnv);
    expect(await grantRow(desktop.desktopId, device.deviceId)).toMatchObject({ state: "pending", permission: null, confirmed_at: null, requested_permission: "read" });
  });

  it("answers 400 for an empty body or a bad permission", async () => {
    const { roomEnv, mine, desktop, device } = await setup();
    await requestGrant(mine, desktop.desktopId, device.deviceId, roomEnv);
    const path = `/api/desktops/${desktop.desktopId}/grants/${device.deviceId}`;
    expect((await authedCall(mine.session, path, json({}, "PUT"), roomEnv)).status).toBe(400);
    expect((await authedCall(mine.session, path, json({ requestedPermission: "root" }, "PUT"), roomEnv)).status).toBe(400);
  });

  it("revoke needs a fresh session (403 fresh_required), while a permission request does not", async () => {
    const { roomEnv, mine, desktop, device } = await setup();
    await requestGrant(mine, desktop.desktopId, device.deviceId, roomEnv);
    const stale = await loginAs(mine.user.id, { fresh: false });
    const path = `/api/desktops/${desktop.desktopId}/grants/${device.deviceId}`;
    const refused = await authedCall(stale, path, json({ state: "revoked" }, "PUT"), roomEnv);
    expect(refused.status).toBe(403);
    expect(await refused.json()).toMatchObject({ code: "fresh_required" });
    expect((await grantRow(desktop.desktopId, device.deviceId))?.state).toBe("pending");
    expect((await authedCall(stale, path, json({ requestedPermission: "read" }, "PUT"), roomEnv)).status).toBe(200);
  });

  it("revokes at once: state revoked, the room closes that device's sockets with 4003 and the desktop is told", async () => {
    const { room, roomEnv, mine, desktop, device } = await setup();
    await requestGrant(mine, desktop.desktopId, device.deviceId, roomEnv);
    await reportState(desktop.token, device.deviceId, { state: "confirmed", permission: "control" }, roomEnv);
    room.calls.revokes.length = 0;
    room.calls.notifies.length = 0;
    const res = await authedCall(mine.session, `/api/desktops/${desktop.desktopId}/grants/${device.deviceId}`, json({ state: "revoked" }, "PUT"), roomEnv);
    expect(res.status).toBe(200);
    expect(((await res.json()) as { grant: { state: string } }).grant.state).toBe("revoked");
    expect(room.calls.revokes).toEqual([{ desktopId: desktop.desktopId, target: { deviceId: device.deviceId }, code: 4003 }]);
    expect(room.calls.notifies).toEqual([desktop.desktopId]);
    // Repeating it is idempotent: no second room revoke.
    await authedCall(mine.session, `/api/desktops/${desktop.desktopId}/grants/${device.deviceId}`, json({ state: "revoked" }, "PUT"), roomEnv);
    // The room is told again when D1 already shows the revocation (R13B-1): idempotent, the room remembers it.
    expect(room.calls.revokes).toHaveLength(2);
    // A requested permission on a revoked grant is a conflict.
    expect((await authedCall(mine.session, `/api/desktops/${desktop.desktopId}/grants/${device.deviceId}`, json({ requestedPermission: "read" }, "PUT"), roomEnv)).status).toBe(409);
  });

  it("answers 404 for another user's desktop or an unknown grant", async () => {
    const { roomEnv, mine, desktop, device } = await setup();
    const other = await newAccount();
    await requestGrant(mine, desktop.desktopId, device.deviceId, roomEnv);
    expect((await authedCall(other.session, `/api/desktops/${desktop.desktopId}/grants/${device.deviceId}`, json({ state: "revoked" }, "PUT"), roomEnv)).status).toBe(404);
    expect((await grantRow(desktop.desktopId, device.deviceId))?.state).toBe("pending");
    expect((await authedCall(mine.session, `/api/desktops/${desktop.desktopId}/grants/${crypto.randomUUID()}`, json({ requestedPermission: "read" }, "PUT"), roomEnv)).status).toBe(404);
  });
});
