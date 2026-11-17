// The real DesktopRoom against the WS-L mock desktop and test client: Noise IK end to end through the relay.
import { describe, expect, it } from "vitest";
import { FrameType, StateMessageSchema } from "@monoptah/protocol";
import { MockDesktop, fingerprint, generateKeyPair } from "@monoptah/mock-desktop";
import { TestClient } from "@monoptah/mock-desktop/test-client";
import { WorkerdFrameSocket } from "./support/relay-adapter.ts";
import { desktopHeaders, desktopPath, deviceHeaders, devicePath, roomStub, seedRoom, upgrade } from "./support/relay-support.ts";

async function open(path: string, headers: Record<string, string>): Promise<WorkerdFrameSocket> {
  const res = await upgrade(path, headers);
  if (res.status !== 101 || !res.webSocket) throw new Error(`upgrade answered ${res.status}`);
  return new WorkerdFrameSocket(res.webSocket);
}

async function setup(known: boolean) {
  const staticKey = generateKeyPair();
  const f = await seedRoom({ publicKey: Buffer.from(staticKey.publicKey).toString("base64url"), fingerprint: fingerprint(staticKey.publicKey) });
  const desktop = new MockDesktop({ desktopId: f.desktopId, staticKey });
  const deviceKey = generateKeyPair();
  if (known) desktop.setDevice(fingerprint(deviceKey.publicKey), { status: "confirmed", permission: "control" });
  desktop.attach(await open(desktopPath(f.desktopId), desktopHeaders(f.token)));
  const deviceSocket = await open(devicePath(f.desktopId, f.deviceId), deviceHeaders(f.session));
  const client = await TestClient.connect({ socket: deviceSocket, desktopId: f.desktopId, deviceId: f.deviceId, desktopPublicKey: desktop.publicKey, deviceKey });
  return { f, desktop, client, deviceSocket };
}

describe("mock desktop through the DesktopRoom", () => {
  it("completes Noise IK, gets a confirmed state and runs a method call", async () => {
    const { desktop, client } = await setup(true);
    expect(StateMessageSchema.parse(client.state)).toMatchObject({ state: "confirmed", permission: "control" });
    expect((await client.call("workspace.list")).projects.length).toBeGreaterThan(0);
    expect(desktop.sessions).toHaveLength(1);
    desktop.close();
  });

  it("a device unknown to the desktop gets state pending", async () => {
    const { desktop, client } = await setup(false);
    expect(client.state).toEqual({ type: "state", state: "pending" });
    desktop.close();
  });

  it("revoke({deviceId}) closes the client within 1 s and the desktop drops the session", async () => {
    const { f, desktop, client, deviceSocket } = await setup(true);
    const start = Date.now();
    await roomStub(f.desktopId).revoke({ deviceId: f.deviceId }, 4003);
    await expect.poll(() => deviceSocket.closeCode, { timeout: 1000 }).toBe(4003);
    expect(Date.now() - start).toBeLessThan(1000);
    expect(client.closeCode).toBe(4003);
    await expect.poll(() => desktop.sessions.length).toBe(0);
    expect(desktop.relayFrames.some((fr) => fr.type === FrameType.CTRL)).toBe(true);
    desktop.close();
  });
});
