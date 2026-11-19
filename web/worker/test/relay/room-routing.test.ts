import { env } from "cloudflare:workers";
import { describe, expect, it } from "vitest";
import { FrameType } from "@monoptah/protocol";
import { binary, closeAll, connectDesktop, connectDevice, hs1, lastSeen, roomStub, seedExtraDevice, seedRoom, ZERO_ID } from "./support/relay-support.ts";

const OTHER_ID = "99999999-9999-4999-8999-999999999999";

describe("frame routing", () => {
  it("overwrites a client-supplied deviceId with the authenticated one", async () => {
    const f = await seedRoom();
    const desktop = await connectDesktop(f);
    const device = await connectDevice(f, f.deviceId);
    device.send(hs1(OTHER_ID, 5));
    await desktop.until(() => desktop.ofType(FrameType.HS1).length === 1);
    const frame = desktop.ofType(FrameType.HS1)[0];
    expect(frame?.deviceId).toBe(f.deviceId);
    expect(frame?.channel).toBe(5);
    expect(frame?.body).toEqual(binary(8, 7));
    await closeAll(desktop, device);
  });

  it("drops frame types a device may not send", async () => {
    const f = await seedRoom();
    const desktop = await connectDesktop(f);
    const device = await connectDevice(f, f.deviceId);
    device.sendFrame(FrameType.HS2, f.deviceId, 1, binary(4));
    device.sendFrame(FrameType.CTRL, f.deviceId, 1, binary(4));
    device.sendFrame(FrameType.ERROR, f.deviceId, 1, binary(4));
    device.sendFrame(FrameType.BYE, f.deviceId, 2);
    await desktop.until(() => desktop.ofType(FrameType.BYE).length === 1);
    expect(desktop.frames.filter((fr) => fr.type !== FrameType.CTRL).map((fr) => fr.type)).toEqual([FrameType.BYE]);
    await closeAll(desktop, device);
  });

  it("routes desktop frames by the header deviceId to every tab of that device only", async () => {
    const f = await seedRoom();
    const other = await seedExtraDevice(f);
    const desktop = await connectDesktop(f);
    const tabA = await connectDevice(f, f.deviceId);
    const tabB = await connectDevice(f, f.deviceId);
    const stranger = await connectDevice(f, other);
    desktop.sendFrame(FrameType.HS2, f.deviceId, 3, binary(6, 1));
    desktop.sendFrame(FrameType.TRANSPORT, other, 4, binary(6, 2));
    desktop.sendFrame(FrameType.CTRL, f.deviceId, 3, binary(2));
    await tabA.until(() => tabA.ofType(FrameType.HS2).length === 1);
    await tabB.until(() => tabB.ofType(FrameType.HS2).length === 1);
    await stranger.until(() => stranger.ofType(FrameType.TRANSPORT).length === 1);
    expect(tabA.ofType(FrameType.TRANSPORT)).toHaveLength(0);
    // The desktop may not send CTRL: it is dropped.
    expect(tabA.ofType(FrameType.CTRL).every((fr) => fr.channel === 0)).toBe(true);
    expect(stranger.ofType(FrameType.HS2)).toHaveLength(0);
    await closeAll(desktop, tabA, tabB, stranger);
  });

  it("drops a desktop frame for a device that is not connected", async () => {
    const f = await seedRoom();
    const desktop = await connectDesktop(f);
    desktop.sendFrame(FrameType.TRANSPORT, OTHER_ID, 1, binary(4));
    desktop.send("ping");
    await desktop.until(() => desktop.texts.includes("pong"));
    expect(desktop.closeCode).toBeNull();
    await closeAll(desktop);
  });

  it("answers a device frame with ERROR desktop_offline on the same channel when no desktop is connected", async () => {
    const f = await seedRoom();
    const device = await connectDevice(f, f.deviceId);
    device.send(hs1(OTHER_ID, 9));
    await device.until(() => device.ofType(FrameType.ERROR).length === 1);
    const error = device.ofType(FrameType.ERROR)[0];
    expect(error?.channel).toBe(9);
    expect(error?.deviceId).toBe(f.deviceId);
    expect(JSON.parse(new TextDecoder().decode(error?.body))).toEqual({ code: "desktop_offline" });
    await closeAll(device);
  });

  it("answers the text ping with pong", async () => {
    const f = await seedRoom();
    const device = await connectDevice(f, f.deviceId);
    device.send("ping");
    await device.until(() => device.texts.includes("pong"));
    await closeAll(device);
  });
});

describe("CTRL frames, presence and last_seen_at", () => {
  it("sends presence, device joined and left, and grants frames", async () => {
    const f = await seedRoom();
    const device = await connectDevice(f, f.deviceId);
    await device.until(() => device.ctrl().length === 1);
    expect(device.ctrl()).toEqual([{ t: "presence", desktop: "offline" }]);

    const desktop = await connectDesktop(f);
    await device.until(() => device.ctrl().length === 2);
    expect(device.ctrl()[1]).toEqual({ t: "presence", desktop: "online" });
    await desktop.until(() => desktop.ctrl().length === 1);
    expect(desktop.ctrl()).toEqual([{ t: "grants" }]);

    const second = await connectDevice(f, f.deviceId);
    await second.until(() => second.ctrl().length === 1);
    expect(second.ctrl()[0]).toEqual({ t: "presence", desktop: "online" });
    await desktop.until(() => desktop.ctrl().length === 2);
    expect(desktop.ctrl()[1]).toEqual({ t: "device", id: f.deviceId, event: "joined" });

    await roomStub(f.desktopId).notifyGrants();
    await desktop.until(() => desktop.ctrl().length === 3);
    expect(desktop.ctrl()[2]).toEqual({ t: "grants" });

    // Closing one of two tabs is not "left"; closing the last one is.
    device.ws.close(1000);
    await device.closed();
    second.ws.close(1000);
    await desktop.until(() => desktop.ctrl().some((c) => (c as { event?: string }).event === "left"));
    expect(desktop.ctrl().filter((c) => (c as { event?: string }).event === "left")).toEqual([{ t: "device", id: f.deviceId, event: "left" }]);

    desktop.ws.close(1000);
    await closeAll(desktop);
  });

  it("tells devices the desktop went offline when it closes", async () => {
    const f = await seedRoom();
    const desktop = await connectDesktop(f);
    const device = await connectDevice(f, f.deviceId);
    await device.until(() => device.ctrl().length === 1);
    desktop.ws.close(1000);
    await device.until(() => device.ctrl().length === 2);
    expect(device.ctrl()[1]).toEqual({ t: "presence", desktop: "offline" });
    await closeAll(device);
  });

  it("presence() follows the desktop socket", async () => {
    const f = await seedRoom();
    expect(await roomStub(f.desktopId).presence()).toEqual({ online: false });
    const desktop = await connectDesktop(f);
    await desktop.until(() => desktop.ctrl().length === 1);
    expect(await roomStub(f.desktopId).presence()).toEqual({ online: true });
    desktop.ws.close(1000);
    await desktop.closed();
    await expect.poll(async () => (await roomStub(f.desktopId).presence()).online).toBe(false);
  });

  it("writes last_seen_at when sockets close, not before", async () => {
    const f = await seedRoom();
    const desktop = await connectDesktop(f);
    const device = await connectDevice(f, f.deviceId);
    device.send(hs1(ZERO_ID));
    desktop.send("ping");
    await desktop.until(() => desktop.texts.includes("pong") && desktop.ofType(FrameType.HS1).length === 1);
    expect(await lastSeen("desktops", f.desktopId)).toBeNull();
    expect(await lastSeen("devices", f.deviceId)).toBeNull();
    device.ws.close(1000);
    await expect.poll(() => lastSeen("devices", f.deviceId)).not.toBeNull();
    expect(await lastSeen("desktops", f.desktopId)).toBeNull();
    desktop.ws.close(1000);
    await expect.poll(() => lastSeen("desktops", f.desktopId)).not.toBeNull();
  });

  it("rewrites last_seen_at only when the stored value is an hour old", async () => {
    const f = await seedRoom();
    const nowSec = Math.floor(Date.now() / 1000);
    await env.DB.prepare("UPDATE desktops SET last_seen_at = ? WHERE id = ?").bind(nowSec - 100, f.desktopId).run();
    await env.DB.prepare("UPDATE devices SET last_seen_at = ? WHERE id = ?").bind(nowSec - 7200, f.deviceId).run();
    const desktop = await connectDesktop(f);
    const device = await connectDevice(f, f.deviceId);
    await device.until(() => device.ctrl().length === 1);
    device.ws.close(1000);
    // The stale device value is replaced, the fresh desktop value is left alone.
    await expect.poll(async () => ((await lastSeen("devices", f.deviceId)) ?? 0) >= nowSec - 5).toBe(true);
    desktop.ws.close(1000);
    await desktop.closed();
    await expect.poll(async () => (await roomStub(f.desktopId).presence()).online).toBe(false);
    expect(await lastSeen("desktops", f.desktopId)).toBe(nowSec - 100);
  });
});

describe("a device socket that closes without BYE", () => {
  it("sends the desktop a BYE for each channel it used, so a reload cannot leak desktop sessions", async () => {
    const f = await seedRoom();
    const desktop = await connectDesktop(f);
    const tab = await connectDevice(f, f.deviceId);
    tab.send(hs1(f.deviceId, 3));
    tab.send(hs1(f.deviceId, 4));
    await desktop.until(() => desktop.ofType(FrameType.HS1).length === 2);

    tab.ws.close(1001);
    await desktop.until(() => desktop.ofType(FrameType.BYE).length === 2);
    expect(desktop.ofType(FrameType.BYE).map((fr) => [fr.deviceId, fr.channel]).sort()).toEqual([
      [f.deviceId, 3],
      [f.deviceId, 4],
    ]);
    await desktop.until(() => desktop.ctrl().some((c) => (c as { event?: string }).event === "left"));
    await closeAll(desktop);
  });

  it("keeps a channel that a newer socket of the same device already took over", async () => {
    const f = await seedRoom();
    const desktop = await connectDesktop(f);
    const oldTab = await connectDevice(f, f.deviceId);
    oldTab.send(hs1(f.deviceId, 1));
    oldTab.send(hs1(f.deviceId, 2));
    await desktop.until(() => desktop.ofType(FrameType.HS1).length === 2);
    // The reloaded page connects before the old socket is noticed gone and reuses channel 1.
    const newTab = await connectDevice(f, f.deviceId);
    newTab.send(hs1(f.deviceId, 1));
    await desktop.until(() => desktop.ofType(FrameType.HS1).length === 3);

    oldTab.ws.close(1001);
    await desktop.until(() => desktop.ofType(FrameType.BYE).length === 1);
    // Only channel 2 is abandoned; channel 1 belongs to the new socket. No `left`: the device is still there.
    desktop.send("ping");
    await desktop.until(() => desktop.texts.includes("pong"));
    expect(desktop.ofType(FrameType.BYE).map((fr) => [fr.deviceId, fr.channel])).toEqual([[f.deviceId, 2]]);
    expect(desktop.ctrl().some((c) => (c as { event?: string }).event === "left")).toBe(false);
    await closeAll(desktop, newTab);
  });
});
