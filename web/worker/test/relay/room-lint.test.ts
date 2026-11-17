import { afterEach, describe, expect, it, vi } from "vitest";
import { FrameType } from "@monoptah/protocol";
import bucketSrc from "../../src/relay/bucket.ts?raw";
import roomSrc from "../../src/relay/DesktopRoom.ts?raw";
import roomsSrc from "../../src/relay/rooms.ts?raw";
import { closeAll, connectDesktop, connectDevice, seedRoom } from "./support/relay-support.ts";

afterEach(() => vi.restoreAllMocks());

describe("hibernation hygiene", () => {
  it("src/relay has no timers or alarms", () => {
    for (const [name, source] of Object.entries({ "DesktopRoom.ts": roomSrc, "bucket.ts": bucketSrc, "rooms.ts": roomsSrc })) {
      expect(source, name).not.toMatch(/setTimeout|setInterval|setAlarm|alarm\(/);
    }
  });

  it("logs contain no frame bytes, tokens or cookies", async () => {
    const lines: string[] = [];
    for (const method of ["log", "info", "warn", "error", "debug"] as const) {
      vi.spyOn(console, method).mockImplementation((...args: unknown[]) => void lines.push(args.map(String).join(" ")));
    }
    const f = await seedRoom();
    const desktop = await connectDesktop(f);
    const device = await connectDevice(f, f.deviceId);
    const marker = "SECRETBODYMARKER";
    device.sendFrame(FrameType.TRANSPORT, f.deviceId, 1, new TextEncoder().encode(marker));
    await desktop.until(() => desktop.ofType(FrameType.TRANSPORT).length === 1);
    device.send("bad text");
    await device.closed();
    await closeAll(desktop);
    await desktop.closed();
    const all = lines.join("\n");
    expect(all).toContain("relay close");
    expect(all).not.toContain(marker);
    expect(all).not.toContain(f.token);
    expect(all).not.toContain(f.session.token);
    expect(all).not.toContain(f.session.idHash);
  });
});
