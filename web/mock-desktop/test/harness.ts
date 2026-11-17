import { vi } from "vitest";
import { InMemoryRelay, MockDesktop, fingerprint, generateKeyPair, type KeyPair, type MockDesktopOptions } from "../src/index.ts";
import { TestClient, type TestClientOptions } from "../src/client.ts";
import type { FrameSocket } from "../src/index.ts";

export const DESKTOP_ID = "11111111-1111-4111-8111-111111111111";
export const DEVICE_ID = "22222222-2222-4222-8222-222222222222";
export const OTHER_DEVICE_ID = "33333333-3333-4333-8333-333333333333";

export interface Setup {
  relay: InMemoryRelay;
  desktop: MockDesktop;
  client: TestClient;
  deviceKey: KeyPair;
  deviceSocket: FrameSocket;
}

export interface SetupOptions {
  /** Device list entry; `unknown` registers nothing (the desktop treats it as pending). */
  device?: "confirmed" | "pending" | "revoked" | "unknown";
  permission?: "read" | "control";
  desktop?: Partial<MockDesktopOptions>;
  client?: Partial<TestClientOptions>;
}

/** A desktop on an in-memory relay, plus a device client that has handshaken. */
export async function setup(options: SetupOptions = {}): Promise<Setup> {
  const relay = new InMemoryRelay();
  const deviceKey = options.client?.deviceKey ?? generateKeyPair();
  const status = options.device ?? "confirmed";
  const desktop = new MockDesktop({
    desktopId: DESKTOP_ID,
    ...options.desktop,
  });
  if (status !== "unknown") desktop.setDevice(fingerprint(deviceKey.publicKey), { status, permission: options.permission ?? "control" });
  desktop.attach(relay.connectDesktop());
  const deviceSocket = relay.connectDevice(DEVICE_ID);
  const client = await TestClient.connect({
    socket: deviceSocket,
    desktopId: DESKTOP_ID,
    deviceId: DEVICE_ID,
    desktopPublicKey: desktop.publicKey,
    deviceKey,
    ...options.client,
  });
  return { relay, desktop, client, deviceKey, deviceSocket };
}

/** Connects a second channel for the same device and desktop, and returns its client. */
export async function connectChannel(s: Setup, channel: number): Promise<TestClient> {
  return TestClient.connect({
    socket: s.relay.connectDevice(DEVICE_ID),
    desktopId: DESKTOP_ID,
    deviceId: DEVICE_ID,
    channel,
    desktopPublicKey: s.desktop.publicKey,
    deviceKey: s.deviceKey,
  });
}

/** Resolves true when `promise` has settled by now (after the relay has delivered everything). */
export async function isSettled(relay: InMemoryRelay, promise: Promise<unknown>): Promise<boolean> {
  let settled = false;
  promise.then(
    () => (settled = true),
    () => (settled = true),
  );
  await relay.settled();
  await Promise.resolve();
  return settled;
}

export function useFakeClock(): void {
  vi.useFakeTimers();
}
