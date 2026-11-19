import { afterEach, describe, expect, it, vi } from "vitest";
import {
  FrameType,
  PENDING_CONFIRMATION_TIMEOUT_MS,
  StateMessageSchema,
  decodeFrame,
  encodeFrame,
} from "@monoptah/protocol";
import { fingerprint, generateKeyPair } from "../src/index.ts";
import { ClientError, TestClient } from "../src/client.ts";
import { DESKTOP_ID, DEVICE_ID, OTHER_DEVICE_ID, connectChannel, isSettled, setup } from "./harness.ts";

afterEach(() => {
  vi.useRealTimers();
});

describe("Noise IK between the TS client and the mock desktop over the in-memory relay", () => {
  it("completes the handshake and delivers a confirmed state with capabilities", async () => {
    const { client, desktop } = await setup({ desktop: { name: "Example Desktop", version: "9.9.9" } });
    expect(StateMessageSchema.parse(client.state)).toMatchObject({
      type: "state",
      state: "confirmed",
      permission: "control",
      proto: 1,
      desktop: { name: "Example Desktop", version: "9.9.9" },
    });
    expect(client.state?.capabilities).toEqual(expect.arrayContaining(["cloud", "approvals", "cloudStart", "cloudMove"]));
    expect(desktop.sessions).toHaveLength(1);
    expect(desktop.sessions[0]).toMatchObject({ deviceId: DEVICE_ID, channel: 1, state: "confirmed", deviceName: "Test device" });
  });

  it("runs requests and uses a deterministic handshake when ephemeral keys are injected", async () => {
    const eph = (n: number) => new Uint8Array(32).fill(n);
    const a = await setup({ desktop: { ephemeralPrivateKey: () => eph(9), staticKey: generateKeyPair(eph(5)) }, client: { ephemeralPrivateKey: eph(8), deviceKey: generateKeyPair(eph(4)) } });
    expect((await a.client.call("workspace.list")).projects).toHaveLength(2);
    expect(await a.client.call("models.list")).toBeTruthy();
  });

  it("the relay overwrites the deviceId, so a client that claims another id fails the handshake with BYE", async () => {
    const s = await setup();
    const liar = s.relay.connectDevice(OTHER_DEVICE_ID);
    // The client claims DEVICE_ID in its prologue and header, but the relay authenticated this socket as OTHER_DEVICE_ID.
    await expect(
      TestClient.connect({ socket: liar, desktopId: DESKTOP_ID, deviceId: DEVICE_ID, desktopPublicKey: s.desktop.publicKey }),
    ).rejects.toMatchObject({ code: "bye" });
    expect(s.relay.forwarded.some((f) => f.direction === "device->desktop" && f.frame.deviceId === OTHER_DEVICE_ID)).toBe(true);
  });

  it("a wrong pinned desktop key fails the handshake with BYE and no state", async () => {
    const s = await setup();
    const socket = s.relay.connectDevice(OTHER_DEVICE_ID);
    await expect(
      TestClient.connect({ socket, desktopId: DESKTOP_ID, deviceId: OTHER_DEVICE_ID, desktopPublicKey: generateKeyPair().publicKey }),
    ).rejects.toMatchObject({ code: "bye" });
    expect(s.desktop.sessions.map((x) => x.deviceId)).not.toContain(OTHER_DEVICE_ID);
  });

  it("a wrong desktop id in the prologue fails with BYE", async () => {
    const s = await setup();
    const socket = s.relay.connectDevice(OTHER_DEVICE_ID);
    await expect(
      TestClient.connect({ socket, desktopId: "44444444-4444-4444-8444-444444444444", deviceId: OTHER_DEVICE_ID, desktopPublicKey: s.desktop.publicKey }),
    ).rejects.toMatchObject({ code: "bye" });
  });

  it("keeps independent sessions per (deviceId, channel)", async () => {
    const s = await setup();
    const second = await connectChannel(s, 2);
    expect(s.desktop.sessions.map((x) => x.channel).sort()).toEqual([1, 2]);
    expect((await second.call("workspace.list")).projects).toHaveLength(2);
    expect((await s.client.call("workspace.list")).projects).toHaveLength(2);
  });

  it("a new HS1 on an existing pair replaces that session", async () => {
    const s = await setup();
    const again = await TestClient.connect({
      socket: s.deviceSocket,
      desktopId: DESKTOP_ID,
      deviceId: DEVICE_ID,
      channel: 1,
      desktopPublicKey: s.desktop.publicKey,
      deviceKey: s.deviceKey,
    });
    expect(s.desktop.sessions).toHaveLength(1);
    expect(await again.call("models.list")).toBeTruthy();
  });
});

describe("unknown pairs and garbage", () => {
  it("a TRANSPORT frame for an unknown (deviceId, channel) is answered with BYE", async () => {
    const s = await setup();
    const socket = s.relay.connectDevice(OTHER_DEVICE_ID);
    const seen: ReturnType<typeof decodeFrame>[] = [];
    socket.onmessage = (bytes) => seen.push(decodeFrame(bytes));
    socket.send(encodeFrame({ type: FrameType.TRANSPORT, channel: 7, deviceId: OTHER_DEVICE_ID, body: new Uint8Array(40) }));
    await s.relay.settled();
    const bye = seen.filter((f) => f.type === FrameType.BYE);
    expect(bye).toHaveLength(1);
    expect(bye[0]).toMatchObject({ channel: 7, deviceId: OTHER_DEVICE_ID });
  });

  it("a known device on an unknown channel gets BYE too", async () => {
    const s = await setup();
    const seen: ReturnType<typeof decodeFrame>[] = [];
    s.deviceSocket.onmessage = (bytes) => seen.push(decodeFrame(bytes));
    s.deviceSocket.send(encodeFrame({ type: FrameType.TRANSPORT, channel: 99, deviceId: DEVICE_ID, body: new Uint8Array(40) }));
    await s.relay.settled();
    expect(seen.map((f) => [f.type, f.channel])).toEqual([[FrameType.BYE, 99]]);
  });

  it("a ciphertext that fails to authenticate ends the session with BYE", async () => {
    const s = await setup();
    s.client.sendRawTransport(new Uint8Array(64).fill(1));
    await s.relay.settled();
    expect(s.client.byeReceived).toBe(true);
    expect(s.desktop.sessions).toHaveLength(0);
  });

  it("a BYE from the device ends the session without a reply", async () => {
    const s = await setup();
    s.client.bye();
    await s.relay.settled();
    expect(s.desktop.sessions).toHaveLength(0);
    expect(s.client.byeReceived).toBe(false);
  });

  it("an HS1 with garbage gets BYE and creates no session", async () => {
    const s = await setup();
    const socket = s.relay.connectDevice(OTHER_DEVICE_ID);
    const seen: number[] = [];
    socket.onmessage = (bytes) => seen.push(decodeFrame(bytes).type);
    socket.send(encodeFrame({ type: FrameType.HS1, channel: 3, deviceId: OTHER_DEVICE_ID, body: new Uint8Array(96).fill(7) }));
    await s.relay.settled();
    expect(seen.filter((type) => type !== FrameType.CTRL)).toEqual([FrameType.BYE]);
  });
});

describe("device states (architecture.md 5.3)", () => {
  it("pending: the handshake completes, state is pending, requests are ignored until confirmation", async () => {
    const s = await setup({ device: "pending" });
    expect(s.client.state).toEqual({ type: "state", state: "pending" });
    const request = s.client.request("workspace.list");
    expect(await isSettled(s.relay, request)).toBe(false);

    s.desktop.confirmDevice(fingerprint(s.deviceKey.publicKey), "read");
    await s.relay.settled();
    expect(s.client.state).toMatchObject({ state: "confirmed", permission: "read" });
    // The ignored request is not answered later; a new one is.
    expect((await s.client.call("workspace.list")).projects).toHaveLength(2);
  });

  it("an unknown device is treated as pending and appears in the desktop device list", async () => {
    const s = await setup({ device: "unknown" });
    expect(s.client.state).toEqual({ type: "state", state: "pending" });
    const entry = s.desktop.devices.get(fingerprint(s.deviceKey.publicKey));
    expect(entry).toMatchObject({ status: "pending" });
    expect(s.desktop.sessions[0]?.deviceName).toBe("Test device");
  });

  it("pending: BYE after 5 minutes without confirmation", async () => {
    vi.useFakeTimers();
    const s = await setup({ device: "pending" });
    vi.advanceTimersByTime(PENDING_CONFIRMATION_TIMEOUT_MS - 1);
    await s.relay.settled();
    expect(s.client.byeReceived).toBe(false);
    vi.advanceTimersByTime(1);
    await s.relay.settled();
    expect(s.client.byeReceived).toBe(true);
    expect(s.desktop.sessions).toHaveLength(0);
  });

  it("revoked: BYE at the handshake, no state, no session", async () => {
    await expect(setup({ device: "revoked" })).rejects.toMatchObject({ code: "bye" });
  });

  it("revoking a live device sends BYE on every channel", async () => {
    const s = await setup();
    const second = await connectChannel(s, 2);
    s.desktop.revokeDevice(fingerprint(s.deviceKey.publicKey));
    await s.relay.settled();
    expect(s.client.byeReceived).toBe(true);
    expect(second.byeReceived).toBe(true);
    expect(s.desktop.sessions).toHaveLength(0);
    await expect(
      TestClient.connect({ socket: s.relay.connectDevice(DEVICE_ID), desktopId: DESKTOP_ID, deviceId: DEVICE_ID, channel: 5, desktopPublicKey: s.desktop.publicKey, deviceKey: s.deviceKey }),
    ).rejects.toMatchObject({ code: "bye" });
  });

  it("a permission change sends a new state to every live channel", async () => {
    const s = await setup();
    const second = await connectChannel(s, 2);
    s.desktop.setDevice(fingerprint(s.deviceKey.publicKey), { permission: "read" });
    await s.relay.settled();
    expect(s.client.state?.permission).toBe("read");
    expect(second.state?.permission).toBe("read");
  });

  it("a capability toggle sends a new state with the new list and hides cloud fields", async () => {
    const s = await setup();
    s.desktop.setCapabilities(["approvals"]);
    await s.relay.settled();
    expect(s.client.state?.capabilities).toEqual(["approvals"]);
    const list = await s.client.call("workspace.list");
    expect(list.sessions.every((x) => x.target === undefined)).toBe(true);
    expect(list.sessions.some((x) => x.id === "sess-codespace-1")).toBe(false);
    await expect(s.client.call("cloud.instances")).rejects.toMatchObject({ message: "Unsupported mobile operation" });
  });

  it("device.leave makes the desktop forget the device and end its sessions", async () => {
    const s = await setup();
    await s.client.call("device.leave");
    await s.relay.settled();
    expect(s.client.byeReceived).toBe(true);
    expect(s.desktop.devices.has(fingerprint(s.deviceKey.publicKey))).toBe(false);
  });
});

describe("errors", () => {
  it("rejects malformed requests with an error response", async () => {
    const s = await setup();
    const response = await s.client.request("conversation.read", { conversationId: 5 });
    expect(response).toMatchObject({ ok: false, error: "Invalid parameters" });
    expect(await s.client.request("no.such.method")).toMatchObject({ ok: false, error: "Unsupported mobile operation" });
  });

  it("ClientError carries the desktop code", async () => {
    const s = await setup();
    await expect(s.client.call("conversation.read", { conversationId: "nope" })).rejects.toBeInstanceOf(ClientError);
  });
});
