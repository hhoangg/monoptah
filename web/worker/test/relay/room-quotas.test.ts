import { describe, expect, it } from "vitest";
import { FrameType, MAX_FRAME_SIZE } from "@monoptah/protocol";
import { binary, closeAll, connectDesktop, connectDevice, hs1, seedExtraDevice, seedRoom, type Sock } from "./support/relay-support.ts";

describe("room quotas", () => {
  it("a second desktop socket replaces the first, which closes 4009", async () => {
    const f = await seedRoom();
    const first = await connectDesktop(f);
    const device = await connectDevice(f, f.deviceId);
    await device.until(() => device.ctrl().length === 1);
    const second = await connectDesktop(f);
    expect(await first.closed()).toBe(4009);
    await second.until(() => second.ctrl().length === 1);
    // Replacement is not an offline transition: devices only see online again.
    await device.until(() => device.ctrl().length === 2);
    expect(device.ctrl().every((c) => (c as { desktop: string }).desktop === "online")).toBe(true);
    await closeAll(second, device);
  });

  it("closes the 5th socket of one device with 4010", async () => {
    const f = await seedRoom();
    const socks: Sock[] = [];
    for (let i = 0; i < 4; i++) socks.push(await connectDevice(f, f.deviceId));
    const fifth = await connectDevice(f, f.deviceId);
    expect(await fifth.closed()).toBe(4010);
    expect(socks.every((s) => s.closeCode === null)).toBe(true);
    await closeAll(...socks);
  });

  it("closes the 17th device socket of a room with 4010", async () => {
    const f = await seedRoom();
    const socks: Sock[] = [];
    for (let d = 0; d < 4; d++) {
      const id = d === 0 ? f.deviceId : await seedExtraDevice(f);
      for (let i = 0; i < 4; i++) socks.push(await connectDevice(f, id));
    }
    const extra = await seedExtraDevice(f);
    const seventeenth = await connectDevice(f, extra);
    expect(await seventeenth.closed()).toBe(4010);
    await closeAll(...socks);
  });

  it("closes a frame over MAX_FRAME_SIZE with 1009", async () => {
    const f = await seedRoom();
    const device = await connectDevice(f, f.deviceId);
    device.send(binary(MAX_FRAME_SIZE + 1, 1));
    expect(await device.closed()).toBe(1009);
  });

  it("closes other text with 4012", async () => {
    const f = await seedRoom();
    const device = await connectDevice(f, f.deviceId);
    device.send("hello");
    expect(await device.closed()).toBe(4012);
  });

  it("closes a bad frame with 4012 (too short, bad version, unknown type)", async () => {
    const f = await seedRoom();
    const bad: Uint8Array[] = [binary(10, 1), (() => { const b = hs1(f.deviceId); b[0] = 2; return b; })(), (() => { const b = hs1(f.deviceId); b[1] = 0x7f; return b; })()];
    for (const frame of bad) {
      const device = await connectDevice(f, f.deviceId);
      device.send(frame);
      expect(await device.closed()).toBe(4012);
    }
  });

  it("applies the frame checks to the desktop too", async () => {
    const f = await seedRoom();
    const desktop = await connectDesktop(f);
    desktop.send("nope");
    expect(await desktop.closed()).toBe(4012);
  });

  it("drops frames over the device rate and closes 4008 after 3 breaches", async () => {
    const f = await seedRoom();
    const desktop = await connectDesktop(f);
    const device = await connectDevice(f, f.deviceId);
    for (let i = 0; i < 160; i++) device.send(hs1(f.deviceId, 1, 4));
    expect(await device.closed()).toBe(4008);
    await desktop.until(() => desktop.ofType(FrameType.HS1).length >= 100);
    expect(desktop.ofType(FrameType.HS1).length).toBeLessThan(120);
    // The desktop socket is unaffected.
    expect(desktop.closeCode).toBeNull();
    await closeAll(desktop);
  });
});
