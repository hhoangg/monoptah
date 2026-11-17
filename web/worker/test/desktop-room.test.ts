import { env } from "cloudflare:workers";
import { describe, expect, it } from "vitest";

describe("DesktopRoom stub", () => {
  const stub = () => env.ROOM.get(env.ROOM.idFromName("desktop-test-1"));

  it("presence() reports offline", async () => {
    expect(await stub().presence()).toEqual({ online: false });
  });

  it("revoke() is a no-op for every target shape", async () => {
    const room = stub();
    await expect(room.revoke({ deviceId: "device-1" }, 4003)).resolves.toBeUndefined();
    await expect(room.revoke({ sessionIdHash: "hash-1" }, 4001)).resolves.toBeUndefined();
    await expect(room.revoke({ all: true }, 4003)).resolves.toBeUndefined();
    expect(await room.presence()).toEqual({ online: false });
  });
});
