// R-04 and R-15 (server part): the desktop side of the grant state machine and the 7 day expiry of pending grants.
import { describe, expect, it } from "vitest";
import { cleanup as grantsCleanup } from "../../src/modules/grants/cleanup.ts";
import { authedCall } from "../support/accounts.ts";
import { env, fakeRoom, json, newAccount, pairDesktop, registerDevice, reportState, requestGrant, sha256HexBytes, tokenCall, withEnv, grantRow } from "./support/api-support.ts";

async function setup() {
  const room = fakeRoom();
  const roomEnv = withEnv({ ROOM: room.namespace });
  const mine = await newAccount();
  const desktop = await pairDesktop(mine, { testEnv: roomEnv });
  const device = await registerDevice(mine, "Phone <b>", roomEnv);
  await requestGrant(mine, desktop.desktopId, device.deviceId, roomEnv);
  return { room, roomEnv, mine, desktop, device };
}

describe("GET /api/desktop/grants", () => {
  it("lists a pending grant with deviceName, publicKey and fingerprint (R-15: the desktop can show it offline)", async () => {
    const { roomEnv, desktop, device } = await setup();
    const res = await tokenCall("/api/desktop/grants", desktop.token, {}, roomEnv);
    expect(res.status).toBe(200);
    expect(await res.json()).toEqual({
      grants: [
        {
          deviceId: device.deviceId,
          deviceName: "Phone <b>",
          publicKey: device.publicKey,
          fingerprint: await sha256HexBytes(device.bytes),
          state: "pending",
          requestedPermission: null,
          permission: null,
        },
      ],
    });
  });

  it("reports requestedPermission and permission, and a forgotten device's grant as revoked", async () => {
    const { roomEnv, mine, desktop, device } = await setup();
    await reportState(desktop.token, device.deviceId, { state: "confirmed", permission: "read" }, roomEnv);
    await authedCall(mine.session, `/api/desktops/${desktop.desktopId}/grants/${device.deviceId}`, json({ requestedPermission: "control" }, "PUT"), roomEnv);
    const listed = (await (await tokenCall("/api/desktop/grants", desktop.token, {}, roomEnv)).json()) as { grants: Record<string, unknown>[] };
    expect(listed.grants[0]).toMatchObject({ state: "confirmed", permission: "read", requestedPermission: "control" });
    await authedCall(mine.session, `/api/devices/${device.deviceId}`, { method: "DELETE" }, roomEnv);
    const after = (await (await tokenCall("/api/desktop/grants", desktop.token, {}, roomEnv)).json()) as { grants: Record<string, unknown>[] };
    expect(after.grants[0]).toMatchObject({ state: "revoked" });
  });
});

describe("POST /api/desktop/grants/:deviceId/state", () => {
  it("is the only route that sets confirmed and permission: confirmed needs a permission and sets confirmed_at", async () => {
    const { roomEnv, mine, desktop, device } = await setup();
    expect((await reportState(desktop.token, device.deviceId, { state: "confirmed" }, roomEnv)).status).toBe(400);
    expect((await grantRow(desktop.desktopId, device.deviceId))?.state).toBe("pending");
    const res = await reportState(desktop.token, device.deviceId, { state: "confirmed", permission: "read" }, roomEnv);
    expect(res.status).toBe(200);
    expect(await grantRow(desktop.desktopId, device.deviceId)).toMatchObject({ state: "confirmed", permission: "read" });
    expect((await grantRow(desktop.desktopId, device.deviceId))?.confirmed_at).toBeGreaterThan(0);
    // The user's session cannot reach this route (token auth only).
    const viaSession = await authedCall(mine.session, `/api/desktop/grants/${device.deviceId}/state`, json({ state: "confirmed", permission: "control" }), roomEnv);
    expect(viaSession.status).toBe(401);
  });

  it("clears a request that the applied permission fulfils, and keeps a different one", async () => {
    const { roomEnv, mine, desktop, device } = await setup();
    const path = `/api/desktops/${desktop.desktopId}/grants/${device.deviceId}`;
    await authedCall(mine.session, path, json({ requestedPermission: "control" }, "PUT"), roomEnv);
    await reportState(desktop.token, device.deviceId, { state: "confirmed", permission: "read" }, roomEnv);
    expect((await grantRow(desktop.desktopId, device.deviceId))?.requested_permission).toBe("control");
    await reportState(desktop.token, device.deviceId, { state: "confirmed", permission: "control" }, roomEnv);
    expect(await grantRow(desktop.desktopId, device.deviceId)).toMatchObject({ permission: "control", requested_permission: null });
  });

  it("walks pending to confirmed to pending (permission and confirmed_at cleared) to confirmed to revoked, with audit events by the desktop", async () => {
    const { room, roomEnv, mine, desktop, device } = await setup();
    await reportState(desktop.token, device.deviceId, { state: "confirmed", permission: "control" }, roomEnv);
    expect((await reportState(desktop.token, device.deviceId, { state: "pending" }, roomEnv)).status).toBe(200);
    expect(await grantRow(desktop.desktopId, device.deviceId)).toMatchObject({ state: "pending", permission: null, confirmed_at: null });
    await reportState(desktop.token, device.deviceId, { state: "confirmed", permission: "read" }, roomEnv);
    room.calls.revokes.length = 0;
    expect((await reportState(desktop.token, device.deviceId, { state: "revoked" }, roomEnv)).status).toBe(200);
    expect((await grantRow(desktop.desktopId, device.deviceId))?.state).toBe("revoked");
    const { results } = await env.DB.prepare("SELECT actor_kind, actor_user_id, meta FROM audit_log WHERE action = 'grant.change' AND actor_kind = 'desktop' AND target_id = ? ORDER BY rowid").bind(device.deviceId).all<{ actor_kind: string; actor_user_id: string; meta: string }>();
    expect(results.map((r) => JSON.parse(r.meta).state)).toEqual(["confirmed", "pending", "confirmed", "revoked"]);
    expect(results.every((r) => r.actor_user_id === mine.user.id)).toBe(true);
  });

  it("calls room.revoke for the device when the desktop reports revoked", async () => {
    const { room, roomEnv, desktop, device } = await setup();
    room.calls.revokes.length = 0;
    await reportState(desktop.token, device.deviceId, { state: "revoked" }, roomEnv);
    expect(room.calls.revokes).toEqual([{ desktopId: desktop.desktopId, target: { deviceId: device.deviceId }, code: 4003 }]);
  });

  it("cannot resurrect a grant the server revoked: confirmed and pending are 409, revoked is accepted", async () => {
    const { room, roomEnv, mine, desktop, device } = await setup();
    await authedCall(mine.session, `/api/desktops/${desktop.desktopId}/grants/${device.deviceId}`, json({ state: "revoked" }, "PUT"), roomEnv);
    for (const body of [{ state: "confirmed", permission: "control" }, { state: "pending" }]) {
      const res = await reportState(desktop.token, device.deviceId, body, roomEnv);
      expect(res.status).toBe(409);
      expect(await res.json()).toMatchObject({ code: "conflict" });
    }
    expect(await grantRow(desktop.desktopId, device.deviceId)).toMatchObject({ state: "revoked", permission: null });
    room.calls.revokes.length = 0;
    expect((await reportState(desktop.token, device.deviceId, { state: "revoked" }, roomEnv)).status).toBe(200);
    // Idempotent room call (the server revoke may not have reached the room), no D1 write.
    expect(room.calls.revokes).toEqual([{ desktopId: desktop.desktopId, target: { deviceId: device.deviceId }, code: 4003 }]);
  });

  it("answers 404 for a device with no grant on this desktop and 400 for a bad state", async () => {
    const { roomEnv, mine, desktop } = await setup();
    const stranger = await registerDevice(mine, "No grant");
    expect((await reportState(desktop.token, stranger.deviceId, { state: "revoked" }, roomEnv)).status).toBe(404);
    expect((await reportState(desktop.token, crypto.randomUUID(), { state: "pending" }, roomEnv)).status).toBe(404);
    expect((await reportState(desktop.token, stranger.deviceId, { state: "frozen" }, roomEnv)).status).toBe(400);
    expect((await reportState(desktop.token, stranger.deviceId, { state: "confirmed", permission: "root" }, roomEnv)).status).toBe(400);
  });
});

describe("pending grants expire after 7 days (R-15 cron)", () => {
  it("deletes pending grants older than 7 days and keeps newer pending, confirmed and revoked ones", async () => {
    const mine = await newAccount();
    const desktop = await pairDesktop(mine);
    const devices: Awaited<ReturnType<typeof registerDevice>>[] = [];
    for (let i = 0; i < 5; i += 1) devices.push(await registerDevice(mine, `D${i}`));
    for (const d of devices) await requestGrant(mine, desktop.desktopId, d.deviceId);
    const now = Math.floor(Date.now() / 1000);
    const eight = now - 8 * 86_400;
    const six = now - 6 * 86_400;
    const set = (state: string, updatedAt: number, index: number) =>
      env.DB.prepare("UPDATE device_grants SET state = ?, updated_at = ? WHERE device_id = ?").bind(state, updatedAt, devices[index]?.deviceId).run();
    await set("pending", eight, 0);
    await set("pending", six, 1);
    await set("confirmed", eight, 2);
    await set("revoked", eight, 3);
    await grantsCleanup(env, now);
    const left = await env.DB.prepare("SELECT device_id FROM device_grants WHERE desktop_id = ?").bind(desktop.desktopId).all<{ device_id: string }>();
    const ids = new Set(left.results.map((r) => r.device_id));
    expect(ids.has(devices[0]?.deviceId as string)).toBe(false);
    for (const i of [1, 2, 3, 4]) expect(ids.has(devices[i]?.deviceId as string)).toBe(true);
  });
});
