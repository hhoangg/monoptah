import { describe, expect, it } from "vitest";
import { FrameType } from "@monoptah/protocol";
import { loginAs } from "../support/accounts.ts";
import { closeAll, connectDesktop, connectDevice, hs1, roomStub, seedExtraDevice, seedRoom, type Sock } from "./support/relay-support.ts";

async function timed<T>(promise: Promise<T>): Promise<number> {
  const start = Date.now();
  await promise;
  return Date.now() - start;
}

describe("room.revoke", () => {
  it("by deviceId closes that device with 4003 within 1 s, sends BYE per channel and device left", async () => {
    const f = await seedRoom();
    const other = await seedExtraDevice(f);
    const desktop = await connectDesktop(f);
    const a = await connectDevice(f, f.deviceId);
    const b = await connectDevice(f, f.deviceId);
    const keep = await connectDevice(f, other);
    a.send(hs1(f.deviceId, 1));
    b.send(hs1(f.deviceId, 2));
    keep.send(hs1(other, 1));
    await desktop.until(() => desktop.ofType(FrameType.HS1).length === 3);

    const start = Date.now();
    await roomStub(f.desktopId).revoke({ deviceId: f.deviceId }, 4003);
    expect(await a.closed(1000)).toBe(4003);
    expect(await b.closed(1000)).toBe(4003);
    expect(Date.now() - start).toBeLessThan(1000);

    await desktop.until(() => desktop.ofType(FrameType.BYE).length === 2);
    expect(desktop.ofType(FrameType.BYE).map((fr) => [fr.deviceId, fr.channel]).sort()).toEqual([
      [f.deviceId, 1],
      [f.deviceId, 2],
    ]);
    await desktop.until(() => desktop.ctrl().some((c) => (c as { event?: string }).event === "left"));
    expect(keep.closeCode).toBeNull();
    await closeAll(desktop, keep);
  });

  it("with {all: true} closes devices and the desktop with 4011 within 1 s", async () => {
    const f = await seedRoom();
    const desktop = await connectDesktop(f);
    const device = await connectDevice(f, f.deviceId);
    await device.until(() => device.ctrl().length === 1);
    const ms = await timed(roomStub(f.desktopId).revoke({ all: true }, 4011));
    expect(ms).toBeLessThan(1000);
    expect(await device.closed(1000)).toBe(4011);
    expect(await desktop.closed(1000)).toBe(4011);
    await expect.poll(async () => (await roomStub(f.desktopId).presence()).online).toBe(false);
  });

  it("by sessionIdHash closes only the sockets of that session", async () => {
    const f = await seedRoom();
    const second = await loginAs(f.userId);
    const other = await seedExtraDevice(f);
    const mine = await connectDevice(f, f.deviceId);
    const mine2 = await connectDevice(f, f.deviceId);
    const theirs = await connectDevice({ desktopId: f.desktopId, session: second }, other);
    const desktop = await connectDesktop(f);
    await roomStub(f.desktopId).revoke({ sessionIdHash: f.session.idHash }, 4001);
    expect(await mine.closed(1000)).toBe(4001);
    expect(await mine2.closed(1000)).toBe(4001);
    await theirs.until(() => theirs.ctrl().length > 0);
    expect(theirs.closeCode).toBeNull();
    expect(desktop.closeCode).toBeNull();
    await closeAll(theirs, desktop);
  });

  it("by sessionIdHashes closes several sessions in one call", async () => {
    const f = await seedRoom();
    const s2 = await loginAs(f.userId);
    const s3 = await loginAs(f.userId);
    const d2 = await seedExtraDevice(f);
    const d3 = await seedExtraDevice(f);
    const a = await connectDevice(f, f.deviceId);
    const b = await connectDevice({ desktopId: f.desktopId, session: s2 }, d2);
    const c = await connectDevice({ desktopId: f.desktopId, session: s3 }, d3);
    await roomStub(f.desktopId).revoke({ sessionIdHashes: [f.session.idHash, s2.idHash] }, 4001);
    expect(await a.closed(1000)).toBe(4001);
    expect(await b.closed(1000)).toBe(4001);
    await c.until(() => c.ctrl().length > 0);
    expect(c.closeCode).toBeNull();
    await closeAll(c);
  });

  it("is a no-op for a room with no matching sockets", async () => {
    const f = await seedRoom();
    await expect(roomStub(f.desktopId).revoke({ deviceId: "nobody" }, 4003)).resolves.toBeUndefined();
    const socks: Sock[] = [];
    expect(socks).toHaveLength(0);
  });
});
