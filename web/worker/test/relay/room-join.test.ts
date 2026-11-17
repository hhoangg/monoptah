import { env } from "cloudflare:workers";
import { describe, expect, it } from "vitest";
import { createTestUser, loginAs } from "../support/accounts.ts";
import {
  addDesktop,
  addDevice,
  addGrant,
  addToken,
  closeAll,
  connect,
  connectDesktop,
  connectDevice,
  desktopHeaders,
  desktopPath,
  deviceHeaders,
  devicePath,
  seedRoom,
  upgrade,
} from "./support/relay-support.ts";

async function closeCodeOf(path: string, headers: Record<string, string>): Promise<number> {
  const sock = await connect(path, headers);
  return sock.closed();
}

describe("device join", () => {
  it("answers 426 without an Upgrade header", async () => {
    const f = await seedRoom();
    const res = await fetch_(devicePath(f.desktopId, f.deviceId), deviceHeaders(f.session));
    expect(res.status).toBe(426);
    expect(await res.json()).toMatchObject({ code: "invalid" });
  });

  it("joins with a session, an owned desktop and a confirmed grant", async () => {
    const f = await seedRoom();
    const sock = await connectDevice(f, f.deviceId);
    await sock.until(() => sock.ctrl().length > 0);
    expect(sock.ctrl()[0]).toEqual({ t: "presence", desktop: "offline" });
    await closeAll(sock);
  });

  it("joins with a pending grant", async () => {
    const f = await seedRoom({ grant: "pending" });
    const sock = await connectDevice(f, f.deviceId);
    await sock.until(() => sock.ctrl().length > 0);
    expect(sock.closeCode).toBeNull();
    await closeAll(sock);
  });

  it("is refused with 401 without a session cookie", async () => {
    const f = await seedRoom();
    const res = await upgrade(devicePath(f.desktopId, f.deviceId), { Origin: env.RP_ORIGIN });
    expect(res.status).toBe(401);
  });

  it("is refused with 403 for a wrong Origin", async () => {
    const f = await seedRoom();
    const res = await upgrade(devicePath(f.desktopId, f.deviceId), { Cookie: f.session.cookie, Origin: "https://evil.example" });
    expect(res.status).toBe(403);
  });

  it("closes 4003 for a revoked grant", async () => {
    const f = await seedRoom({ grant: "revoked" });
    expect(await closeCodeOf(devicePath(f.desktopId, f.deviceId), deviceHeaders(f.session))).toBe(4003);
  });

  it("closes 4003 without any grant row", async () => {
    const f = await seedRoom();
    const other = await addDevice(f.userId);
    expect(await closeCodeOf(devicePath(f.desktopId, other), deviceHeaders(f.session))).toBe(4003);
  });

  it("closes 4003 for a revoked device", async () => {
    const f = await seedRoom();
    const revoked = await addDevice(f.userId, { revoked: true });
    await addGrant(revoked, f.desktopId);
    expect(await closeCodeOf(devicePath(f.desktopId, revoked), deviceHeaders(f.session))).toBe(4003);
  });

  it("closes 4003 for a revoked desktop", async () => {
    const f = await seedRoom();
    const gone = await addDesktop(f.userId, { revoked: true });
    await addGrant(f.deviceId, gone);
    expect(await closeCodeOf(devicePath(gone, f.deviceId), deviceHeaders(f.session))).toBe(4003);
  });

  it("closes 4003 for another user's device or desktop", async () => {
    const f = await seedRoom();
    const stranger = await seedRoom();
    expect(await closeCodeOf(devicePath(f.desktopId, stranger.deviceId), deviceHeaders(f.session))).toBe(4003);
    expect(await closeCodeOf(devicePath(stranger.desktopId, f.deviceId), deviceHeaders(f.session))).toBe(4003);
    // A grant row alone is not enough when the device belongs to someone else.
    await addGrant(stranger.deviceId, f.desktopId);
    expect(await closeCodeOf(devicePath(f.desktopId, stranger.deviceId), deviceHeaders(f.session))).toBe(4003);
  });

  it("closes 4003 for a missing or malformed device id", async () => {
    const f = await seedRoom();
    expect(await closeCodeOf(`/relay/device/${f.desktopId}`, deviceHeaders(f.session))).toBe(4003);
    expect(await closeCodeOf(devicePath(f.desktopId, "not-a-uuid"), deviceHeaders(f.session))).toBe(4003);
  });

  it("is refused for a recovery-only session and for a blocked user", async () => {
    const f = await seedRoom();
    const recovery = await loginAs(f.userId, { recoveryOnly: true });
    expect((await upgrade(devicePath(f.desktopId, f.deviceId), deviceHeaders(recovery))).status).toBe(403);
    const blocked = await createTestUser(env, { status: "blocked" });
    const session = await loginAs(blocked.id);
    expect((await upgrade(devicePath(f.desktopId, f.deviceId), deviceHeaders(session))).status).toBe(403);
  });
});

describe("desktop join", () => {
  it("answers 426 without an Upgrade header", async () => {
    const f = await seedRoom();
    const res = await fetch_(desktopPath(f.desktopId), desktopHeaders(f.token));
    expect(res.status).toBe(426);
  });

  it("joins with a valid relay token and receives a grants CTRL frame", async () => {
    const f = await seedRoom();
    const desktop = await connectDesktop(f);
    await desktop.until(() => desktop.ctrl().length > 0);
    expect(desktop.ctrl()[0]).toEqual({ t: "grants" });
    await closeAll(desktop);
  });

  it("closes 4001 without a token", async () => {
    const f = await seedRoom();
    expect(await closeCodeOf(desktopPath(f.desktopId), {})).toBe(4001);
  });

  it("closes 4001 for an unknown token", async () => {
    const f = await seedRoom();
    expect(await closeCodeOf(desktopPath(f.desktopId), desktopHeaders("tok-unknown"))).toBe(4001);
  });

  it("closes 4003 for a revoked token", async () => {
    const f = await seedRoom();
    const token = await addToken(f.desktopId, { revoked: true });
    expect(await closeCodeOf(desktopPath(f.desktopId), desktopHeaders(token))).toBe(4003);
  });

  it("closes 4003 for a revoked desktop", async () => {
    const f = await seedRoom();
    const id = await addDesktop(f.userId, { revoked: true });
    const token = await addToken(id);
    expect(await closeCodeOf(desktopPath(id), desktopHeaders(token))).toBe(4003);
  });

  it("closes 4001 for another desktop's token", async () => {
    const a = await seedRoom();
    const b = await seedRoom();
    expect(await closeCodeOf(desktopPath(a.desktopId), desktopHeaders(b.token))).toBe(4001);
  });

  it("closes 4001 for a token without the relay scope", async () => {
    const f = await seedRoom();
    const token = await addToken(f.desktopId, { scope: "push grants" });
    expect(await closeCodeOf(desktopPath(f.desktopId), desktopHeaders(token))).toBe(4001);
  });

  it("closes 4011 when the owner is blocked", async () => {
    const owner = await createTestUser(env, { status: "blocked" });
    const id = await addDesktop(owner.id);
    const token = await addToken(id);
    expect(await closeCodeOf(desktopPath(id), desktopHeaders(token))).toBe(4011);
  });

  it("ignores a session cookie on the desktop route", async () => {
    const f = await seedRoom();
    expect(await closeCodeOf(desktopPath(f.desktopId), { Cookie: f.session.cookie })).toBe(4001);
  });
});

async function fetch_(path: string, headers: Record<string, string>): Promise<Response> {
  const { SELF } = await import("cloudflare:test");
  return SELF.fetch(`${env.RP_ORIGIN}${path}`, { headers });
}
