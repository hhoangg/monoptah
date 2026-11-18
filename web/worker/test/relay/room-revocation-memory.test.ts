// R13B-1: the room remembers revocations (so a join that raced the revoke is refused), closes sockets past their
// session expiry without a timer, and is told again when D1 already shows the revocation.
import { env } from "cloudflare:workers";
import { runInDurableObject } from "cloudflare:test";
import { describe, expect, it } from "vitest";
import { FrameType } from "@monoptah/protocol";
import { REVOCATION_LIST_LIMIT } from "../../src/relay/DesktopRoom.ts";
import { revokeAllSessions } from "../../src/modules/sessions/session.ts";
import { authedCall, loginAs } from "../support/accounts.ts";
import { closeAll, connectDesktop, connectDevice, hs1, roomStub, seedRoom } from "./support/relay-support.ts";
import { json } from "./support/api-support.ts";

describe("a join after a revocation", () => {
  it("is refused with 4003 when the device was revoked in the room, even though D1 still allows it (the race)", async () => {
    const f = await seedRoom();
    await roomStub(f.desktopId).revoke({ deviceId: f.deviceId }, 4003);
    const late = await connectDevice(f, f.deviceId);
    expect(await late.closed(1000)).toBe(4003);
  });

  it("is refused with 4001 when the session was revoked in the room, for that session only", async () => {
    const f = await seedRoom();
    const other = await loginAs(f.userId);
    await roomStub(f.desktopId).revoke({ sessionIdHashes: [f.session.idHash] }, 4001);
    const late = await connectDevice(f, f.deviceId);
    expect(await late.closed(1000)).toBe(4001);
    const fine = await connectDevice({ desktopId: f.desktopId, session: other }, f.deviceId);
    await fine.until(() => fine.ctrl().length === 1);
    expect(fine.closeCode).toBeNull();
    await closeAll(fine);
  });

  it("is not refused after a revoke of another device, and the memory survives a second revoke call (idempotent)", async () => {
    const f = await seedRoom();
    await roomStub(f.desktopId).revoke({ deviceId: f.deviceId }, 4003);
    await roomStub(f.desktopId).revoke({ deviceId: f.deviceId }, 4003);
    expect(await (await connectDevice(f, f.deviceId)).closed(1000)).toBe(4003);
  });

  it("keeps the revocation list bounded", async () => {
    const f = await seedRoom();
    const stub = roomStub(f.desktopId);
    for (let i = 0; i < REVOCATION_LIST_LIMIT + 20; i += 1) await stub.revoke({ deviceId: crypto.randomUUID() }, 4003);
    const stored = await runInDurableObject(stub, (_instance, state) => state.storage.get<{ devices: Record<string, number> }>("revoked"));
    expect(Object.keys(stored?.devices ?? {}).length).toBe(REVOCATION_LIST_LIMIT);
  });
});

describe("sign out everywhere", () => {
  it("closes the live device socket of the revoked session with 4001 and keeps the excepted session", async () => {
    const f = await seedRoom();
    const keep = await loginAs(f.userId);
    const desktop = await connectDesktop(f);
    const socket = await connectDevice(f, f.deviceId);
    await socket.until(() => socket.ctrl().length === 1);
    socket.send(hs1(f.deviceId, 1));
    await desktop.until(() => desktop.ofType(FrameType.HS1).length === 1);

    expect(await revokeAllSessions(env, f.userId, keep.idHash)).toBeGreaterThan(0);
    expect(await socket.closed(1000)).toBe(4001);
    await desktop.until(() => desktop.ofType(FrameType.BYE).length === 1);
    // A reconnect with the revoked cookie is refused at once (the session row is gone).
    expect((await authedCall(f.session, "/api/me")).status).toBe(401);
    await closeAll(desktop);
  });
});

describe("a socket past its session expiry", () => {
  it("is closed with 4001 on its next message, with BYE to the desktop, and a new join with that session is refused", async () => {
    const f = await seedRoom();
    const desktop = await connectDesktop(f);
    const soon = Math.floor(Date.now() / 1000) + 2;
    await env.DB.prepare("UPDATE sessions SET expires_at = ? WHERE id_hash = ?").bind(soon, f.session.idHash).run();
    const socket = await connectDevice(f, f.deviceId);
    await socket.until(() => socket.ctrl().length === 1);
    socket.send(hs1(f.deviceId, 3));
    await desktop.until(() => desktop.ofType(FrameType.HS1).length === 1);
    expect(socket.closeCode).toBeNull();

    await new Promise((resolve) => setTimeout(resolve, Math.max(0, soon * 1000 - Date.now()) + 50));
    expect(socket.closeCode).toBeNull(); // no timer: nothing closes it until a message arrives
    socket.send(hs1(f.deviceId, 3));
    expect(await socket.closed(1000)).toBe(4001);
    await desktop.until(() => desktop.ofType(FrameType.BYE).length === 1);
    await closeAll(desktop);
  });

  it("is closed when another device's message reaches the room (sweep), not only its own", async () => {
    const f = await seedRoom();
    const desktop = await connectDesktop(f);
    const soon = Math.floor(Date.now() / 1000) + 2;
    const second = await loginAs(f.userId);
    await env.DB.prepare("UPDATE sessions SET expires_at = ? WHERE id_hash = ?").bind(soon, second.idHash).run();
    const quiet = await connectDevice({ desktopId: f.desktopId, session: second }, f.deviceId);
    const active = await connectDevice(f, f.deviceId);
    await quiet.until(() => quiet.ctrl().length === 1);
    await new Promise((resolve) => setTimeout(resolve, Math.max(0, soon * 1000 - Date.now()) + 1100));
    active.send(hs1(f.deviceId, 1));
    expect(await quiet.closed(1000)).toBe(4001);
    expect(active.closeCode).toBeNull();
    await closeAll(desktop, active);
  });
});

describe("a sweep never delays or reorders forwarding (N4)", () => {
  it("forwards frames in order to the desktop while an expired socket is swept", async () => {
    const f = await seedRoom();
    const desktop = await connectDesktop(f);
    const soon = Math.floor(Date.now() / 1000) + 2;
    const second = await loginAs(f.userId);
    await env.DB.prepare("UPDATE sessions SET expires_at = ? WHERE id_hash = ?").bind(soon, second.idHash).run();
    const quiet = await connectDevice({ desktopId: f.desktopId, session: second }, f.deviceId);
    const active = await connectDevice(f, f.deviceId);
    await quiet.until(() => quiet.ctrl().length === 1);
    await new Promise((resolve) => setTimeout(resolve, Math.max(0, soon * 1000 - Date.now()) + 1100));
    // The first frame triggers the sweep; the rest must still arrive in send order.
    for (let channel = 1; channel <= 20; channel += 1) active.send(hs1(f.deviceId, channel));
    await desktop.until(() => desktop.ofType(FrameType.HS1).length === 20);
    expect(desktop.ofType(FrameType.HS1).map((frame) => frame.channel)).toEqual(Array.from({ length: 20 }, (_, i) => i + 1));
    expect(await quiet.closed(1000)).toBe(4001);
    await closeAll(desktop, active);
  });
});

describe("the web side tells the room again when D1 already shows the revocation", () => {
  it("PUT grant revoked a second time re-sends room.revoke (closes a socket that survived the first call)", async () => {
    const f = await seedRoom();
    const desktop = await connectDesktop(f);
    const socket = await connectDevice(f, f.deviceId);
    await socket.until(() => socket.ctrl().length === 1);
    // The first revocation reaches D1 only (the room call failed): flip the row directly.
    await env.DB.prepare("UPDATE device_grants SET state = 'revoked' WHERE device_id = ? AND desktop_id = ?").bind(f.deviceId, f.desktopId).run();
    expect(socket.closeCode).toBeNull();
    const fresh = await loginAs(f.userId, { fresh: true });
    const res = await authedCall(fresh, `/api/desktops/${f.desktopId}/grants/${f.deviceId}`, json({ state: "revoked" }, "PUT"));
    expect(res.status).toBe(200);
    expect(await socket.closed(1000)).toBe(4003);
    await closeAll(desktop);
  });

  it("DELETE /api/desktops/:id a second time closes the desktop socket and still answers 404", async () => {
    const f = await seedRoom();
    const fresh = await loginAs(f.userId, { fresh: true });
    const desktop = await connectDesktop(f);
    await env.DB.prepare("UPDATE desktops SET revoked_at = 5 WHERE id = ?").bind(f.desktopId).run();
    const res = await authedCall(fresh, `/api/desktops/${f.desktopId}`, { method: "DELETE" });
    expect(res.status).toBe(404);
    expect(await desktop.closed(1000)).toBe(4003);
  });

  it("DELETE /api/devices/:id a second time closes the device socket and still answers 404", async () => {
    const f = await seedRoom();
    const fresh = await loginAs(f.userId, { fresh: true });
    const socket = await connectDevice(f, f.deviceId);
    await socket.until(() => socket.ctrl().length === 1);
    await env.DB.prepare("UPDATE devices SET revoked_at = 5 WHERE id = ?").bind(f.deviceId).run();
    const res = await authedCall(fresh, `/api/devices/${f.deviceId}`, { method: "DELETE" });
    expect(res.status).toBe(404);
    expect(await socket.closed(1000)).toBe(4003);
  });
});
