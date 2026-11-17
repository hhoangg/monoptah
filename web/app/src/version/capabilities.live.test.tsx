// The capability toggle of the desktop reaches a mounted RequireCapability through the real link
// connection, not through a store write (W-22, architecture.md 3.3 step 4).
import { act, render, screen } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { InMemoryRelay, MockDesktop, fingerprint, generateKeyPair } from "@monoptah/mock-desktop";
import { base64UrlEncode } from "@/link/base64url";
import { getLinkConnection, resetLinkConnections } from "@/link/connection";
import { LinkProvider } from "@/link/LinkProvider";
import { RelayConnection } from "@/link/relay";
import { clearLinkStores, getLinkStore } from "@/link/store";
import { RequireCapability } from "./RequireCapability";

const DESKTOP_ID = "11111111-1111-4111-8111-111111111111";
const DEVICE_ID = "22222222-2222-4222-8222-222222222222";

beforeEach(() => {
  vi.useFakeTimers();
});
afterEach(() => {
  resetLinkConnections();
  clearLinkStores();
  vi.useRealTimers();
});

describe("a desktop capability toggle over the live link", () => {
  it("shows and hides a feature without a reconnect", async () => {
    const relay = new InMemoryRelay();
    const deviceKey = generateKeyPair();
    const desktop = new MockDesktop({ desktopId: DESKTOP_ID, capabilities: [] });
    desktop.setDevice(fingerprint(deviceKey.publicKey), { status: "confirmed", permission: "control" });
    desktop.attach(relay.connectDesktop());
    let opens = 0;
    const directory = {
      id: DESKTOP_ID,
      name: "Example Desktop",
      color: null,
      online: true,
      pendingCount: 0,
      os: null,
      appVersion: null,
      lastSeenAt: null,
      publicKey: base64UrlEncode(desktop.publicKey),
      fingerprint: fingerprint(desktop.publicKey),
    };
    getLinkConnection(DESKTOP_ID, {
      ensureDevice: async () => ({ deviceId: DEVICE_ID, publicKey: deviceKey.publicKey, fingerprint: fingerprint(deviceKey.publicKey) }),
      getDeviceKeyPair: async () => deviceKey,
      fetch: (async () => new Response(JSON.stringify(directory), { status: 200, headers: { "Content-Type": "application/json" } })) as typeof fetch,
      checkPin: async () => "match",
      channel: 7,
      openRelay: async (_desktopId, deviceId) => {
        opens += 1;
        return new RelayConnection(relay.connectDevice(deviceId));
      },
    });
    const settle = () =>
      act(async () => {
        await vi.advanceTimersByTimeAsync(0);
        await relay.settled();
        await vi.advanceTimersByTimeAsync(0);
      });
    render(
      <LinkProvider desktopId={DESKTOP_ID}>
        <RequireCapability name="approvals" fallback="hide">
          <p>approvals feature</p>
        </RequireCapability>
      </LinkProvider>,
    );
    await settle();
    expect(getLinkStore(DESKTOP_ID).getState().lastError).toBeNull();
    expect(getLinkStore(DESKTOP_ID).getState().status).toBe("ready");
    expect(screen.queryByText("approvals feature")).toBeNull();

    desktop.setCapabilities(["approvals"]);
    await settle();
    expect(getLinkStore(DESKTOP_ID).getState().capabilities).toEqual(["approvals"]);
    expect(screen.getByText("approvals feature")).toBeInTheDocument();

    desktop.setCapabilities([]);
    await settle();
    expect(screen.queryByText("approvals feature")).toBeNull();
    expect(opens).toBe(1);
  });
});
