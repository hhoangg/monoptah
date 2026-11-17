// @vitest-environment node
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { CLOSE_CODES, CloseCodes, PROTOCOL_VERSION } from "@monoptah/protocol";
import { InMemoryRelay, MockDesktop, fingerprint, generateKeyPair, type KeyPair } from "@monoptah/mock-desktop";
import { base64UrlEncode } from "./base64url.ts";
import {
  BACKOFF_LONG_MS,
  LinkConnection,
  backoffDelay,
  planForClose,
  type LinkConnectionDeps,
} from "./connection.ts";
import type { DeviceIdentity } from "./device.ts";
import { RelayConnectError, RelayConnection } from "./relay.ts";
import { LinkSession, LinkSessionError } from "./session.ts";
import { clearLinkStores, type LinkState, type LinkStatus } from "./store.ts";

const DESKTOP_ID = "11111111-1111-4111-8111-111111111111";
const DEVICE_ID = "22222222-2222-4222-8222-222222222222";

/** A minimal DOM: only what the connection touches. */
class FakeDocument extends EventTarget {
  hidden = false;
}
class FakeWindow extends EventTarget {}

let doc: FakeDocument;
let win: FakeWindow;
let navigatorState: { onLine: boolean };

beforeEach(() => {
  vi.useFakeTimers();
  doc = new FakeDocument();
  win = new FakeWindow();
  navigatorState = { onLine: true };
  vi.stubGlobal("document", doc);
  vi.stubGlobal("navigator", navigatorState);
  vi.stubGlobal("addEventListener", win.addEventListener.bind(win));
  vi.stubGlobal("removeEventListener", win.removeEventListener.bind(win));
});

afterEach(() => {
  vi.unstubAllGlobals();
  vi.useRealTimers();
  vi.restoreAllMocks();
  clearLinkStores();
});

function setHidden(hidden: boolean): void {
  doc.hidden = hidden;
  doc.dispatchEvent(new Event("visibilitychange"));
}

interface Rig {
  relay: InMemoryRelay;
  desktop: MockDesktop;
  deviceKey: KeyPair;
  connection: LinkConnection;
  statuses: LinkStatus[];
  opens: () => number;
  fetchCalls: () => number;
  clock: { now: number };
  resynced: () => number;
  pins: Map<string, Uint8Array>;
  state: () => LinkState;
}

interface RigOptions {
  device?: "confirmed" | "pending" | "revoked";
  desktopOnline?: boolean;
  random?: () => number;
  deps?: Partial<LinkConnectionDeps>;
  pinFn?: LinkConnectionDeps["checkPin"];
  start?: boolean;
}

function rig(options: RigOptions = {}): Rig {
  const relay = new InMemoryRelay();
  const deviceKey = generateKeyPair();
  const desktop = new MockDesktop({ desktopId: DESKTOP_ID, name: "Example Desktop", version: "9.9.9" });
  desktop.setDevice(fingerprint(deviceKey.publicKey), { status: options.device ?? "confirmed", permission: "control" });
  if (options.desktopOnline !== false) desktop.attach(relay.connectDesktop());
  const identity: DeviceIdentity = { deviceId: DEVICE_ID, publicKey: deviceKey.publicKey, fingerprint: fingerprint(deviceKey.publicKey) };
  const pins = new Map<string, Uint8Array>();
  let opens = 0;
  let fetches = 0;
  const clock = { now: Date.now() };
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
  const connection = new LinkConnection(DESKTOP_ID, {
    ensureDevice: async () => identity,
    getDeviceKeyPair: async () => deviceKey,
    fetch: (async () => {
      fetches += 1;
      return new Response(JSON.stringify(directory), { status: 200, headers: { "Content-Type": "application/json" } });
    }) as typeof fetch,
    checkPin:
      options.pinFn ??
      (async (id, key) => {
        const pinned = pins.get(id);
        if (!pinned) {
          pins.set(id, key);
          return "pinned-now";
        }
        return base64UrlEncode(pinned) === base64UrlEncode(key) ? "match" : "changed";
      }),
    openRelay: async (_desktopId, deviceId) => {
      opens += 1;
      return new RelayConnection(relay.connectDevice(deviceId));
    },
    now: () => clock.now,
    random: options.random ?? (() => 0.5),
    channel: 7,
    ...options.deps,
  });
  const statuses: LinkStatus[] = [];
  connection.store.subscribe((state, previous) => {
    if (state.status !== previous.status) statuses.push(state.status);
  });
  let resynced = 0;
  connection.onResynced(() => {
    resynced += 1;
  });
  if (options.start !== false) connection.start();
  return { relay, desktop, deviceKey, connection, statuses, opens: () => opens, fetchCalls: () => fetches, clock, resynced: () => resynced, pins, state: () => connection.getState() };
}

/** Lets pending promises and zero-delay timers run. */
async function settle(ms = 0): Promise<void> {
  await vi.advanceTimersByTimeAsync(ms);
  await vi.advanceTimersByTimeAsync(0);
}

async function ready(options: RigOptions = {}): Promise<Rig> {
  const r = rig(options);
  await settle();
  expect(r.state().status).toBe("ready");
  return r;
}

describe("connect sequence", () => {
  it("goes connecting -> handshaking -> ready and fills the store with public data only", async () => {
    const r = await ready();
    expect(r.statuses).toEqual(["connecting", "handshaking", "ready"]);
    const state = r.state();
    expect(state).toMatchObject({
      permission: "control",
      desktopName: "Example Desktop",
      desktopVersion: "9.9.9",
      proto: PROTOCOL_VERSION,
      presence: "online",
      revoked: false,
      blocked: null,
      lastError: null,
      deviceFingerprint: fingerprint(r.deviceKey.publicKey),
      desktopFingerprint: fingerprint(r.desktop.publicKey),
    });
    expect(state.capabilities).toEqual(expect.arrayContaining(["cloud", "approvals"]));
    expect(r.opens()).toBe(1);
    // The store holds no key material and no request data.
    expect(JSON.stringify(state)).not.toContain(base64UrlEncode(r.deviceKey.privateKey));
    const result = await r.connection.request("workspace.list");
    expect(result.projects.length).toBeGreaterThan(0);
  });

  it("a changed directory key blocks with key_changed before any socket opens", async () => {
    const r = rig({ pinFn: async () => "changed" });
    await settle();
    expect(r.opens()).toBe(0);
    expect(r.state()).toMatchObject({ status: "closed", blocked: "key_changed" });
    await settle(120_000);
    expect(r.opens()).toBe(0);
  });

  it("a directory 401 blocks with login_required and opens no socket", async () => {
    const r = rig({ deps: { fetch: (async () => new Response("{}", { status: 401 })) as typeof fetch } });
    await settle();
    expect(r.opens()).toBe(0);
    expect(r.state()).toMatchObject({ status: "closed", blocked: "login_required" });
  });

  it("a directory 404 ends in closed with lastError and no retry", async () => {
    const r = rig({ deps: { fetch: (async () => new Response("{}", { status: 404 })) as typeof fetch } });
    await settle(120_000);
    expect(r.opens()).toBe(0);
    expect(r.state()).toMatchObject({ status: "closed", blocked: null, lastError: { code: "directory_404" } });
  });

  it("an invalid directory body retries with backoff", async () => {
    let calls = 0;
    const r = rig({
      deps: {
        fetch: (async () => {
          calls += 1;
          return new Response("not json", { status: 200 });
        }) as typeof fetch,
      },
    });
    await settle();
    expect(calls).toBe(1);
    expect(r.state()).toMatchObject({ status: "reconnecting", lastError: { code: "directory_error" } });
    await settle(1000);
    expect(calls).toBe(2);
  });

  it("a desktop that is offline sets presence and retries until it is back", async () => {
    const r = rig({ desktopOnline: false });
    await settle();
    expect(r.state()).toMatchObject({ status: "reconnecting", presence: "offline", lastError: { code: "desktop_offline" } });
    r.desktop.attach(r.relay.connectDesktop());
    await settle(1000);
    expect(r.state().status).toBe("ready");
  });

  it("BYE before any state blocks as revoked and does not retry", async () => {
    const r = rig({ device: "revoked" });
    await settle();
    expect(r.state()).toMatchObject({ status: "closed", blocked: "revoked", revoked: true });
    await settle(120_000);
    expect(r.opens()).toBe(1);
  });

  it("a relay unsupported_version blocks as proto_desktop_older", async () => {
    const r = rig({
      deps: { connectSession: async () => Promise.reject(new LinkSessionError("relay_error", { relayCode: "unsupported_version" })) },
    });
    await settle();
    expect(r.state()).toMatchObject({ status: "closed", blocked: "proto_desktop_older" });
  });

  it("a desktop proto above ours blocks as proto_desktop_newer, at connect and in a later state", async () => {
    const at = rig({ deps: { connectSession: async () => Promise.reject(new LinkSessionError("unsupported_proto")) } });
    await settle();
    expect(at.state()).toMatchObject({ status: "closed", blocked: "proto_desktop_newer" });

    const later = rig({
      deps: {
        connectSession: async (options) => {
          options.onState?.({ type: "state", state: "confirmed", permission: "control", proto: PROTOCOL_VERSION + 1, capabilities: [] });
          return { state: { type: "state", state: "confirmed" }, lastFrameAt: 0, ready: true, ended: false, close() {} } as unknown as LinkSession;
        },
      },
    });
    await settle();
    expect(later.state()).toMatchObject({ status: "closed", blocked: "proto_desktop_newer", proto: PROTOCOL_VERSION + 1 });
  });

  it("a state revoked blocks as revoked", async () => {
    const r = rig({
      deps: {
        connectSession: async (options) => {
          options.onState?.({ type: "state", state: "revoked" });
          return { state: { type: "state", state: "revoked" }, lastFrameAt: 0, ready: false, ended: false, close() {} } as unknown as LinkSession;
        },
      },
    });
    await settle();
    expect(r.state()).toMatchObject({ status: "closed", blocked: "revoked", revoked: true });
  });

  it("a relay that refuses the upgrade (1006) is surfaced in lastError and retried with backoff", async () => {
    const r = rig({ deps: { openRelay: async () => Promise.reject(new RelayConnectError("refused", 1006)) } });
    await settle();
    expect(r.state()).toMatchObject({ status: "reconnecting", lastError: { code: "close_1006", closeCode: 1006 } });
  });

  it("stops without a socket when released and ignores events afterwards", async () => {
    const r = await ready();
    r.connection.stop();
    expect(r.state().status).toBe("idle");
    const opens = r.opens();
    setHidden(false);
    await settle(120_000);
    expect(r.opens()).toBe(opens);
  });
});

describe("close codes", () => {
  it("planForClose follows the device column of CLOSE_CODES", () => {
    for (const entry of CLOSE_CODES) {
      const plan = planForClose(entry.code);
      switch (entry.device.action) {
        case "reconnect_backoff":
        case "reconnect_new_handshake":
          expect(plan).toEqual({ kind: "retry", delay: "backoff" });
          break;
        case "reconnect_once":
        case "retry_once":
          expect(plan).toEqual({ kind: "retry", delay: "once" });
          break;
        case "backoff_60s":
          expect(plan).toEqual({ kind: "retry", delay: "long" });
          break;
        case "login":
          expect(plan).toMatchObject({ kind: "block", reason: "login_required" });
          break;
        case "stop":
          expect(plan.kind).toBe("block");
          break;
      }
    }
    expect(planForClose(4999)).toEqual({ kind: "retry", delay: "backoff" });
  });

  for (const entry of CLOSE_CODES) {
    it(`reacts to ${entry.code} ${entry.name} (${entry.device.action})`, async () => {
      const r = await ready();
      const before = r.opens();
      r.relay.revokeDevice(DEVICE_ID, entry.code);
      await settle();
      const action = entry.device.action;
      if (action === "stop") {
        const expected =
          entry.code === CloseCodes.REVOKED
            ? { blocked: "revoked", revoked: true }
            : entry.code === CloseCodes.BLOCKED
              ? { blocked: "account_blocked", revoked: false }
              : { blocked: "policy", revoked: false };
        expect(r.state()).toMatchObject({ status: "closed", ...expected, lastError: { code: `close_${entry.code}`, closeCode: entry.code } });
        await settle(300_000);
        expect(r.opens()).toBe(before);
      } else if (action === "login") {
        expect(r.state()).toMatchObject({ status: "closed", blocked: "login_required", revoked: false });
        await settle(300_000);
        expect(r.opens()).toBe(before);
      } else if (action === "reconnect_once" || action === "retry_once") {
        // Immediately, once.
        expect(r.opens()).toBe(before + 1);
        expect(r.state().status).toBe("ready");
        // A second drop before the 10 s reset falls back to backoff (0.5 s * 2^0 * 0.75 = 375 ms).
        r.relay.revokeDevice(DEVICE_ID, entry.code);
        await settle(374);
        expect(r.opens()).toBe(before + 1);
        await settle(1);
        expect(r.opens()).toBe(before + 2);
      } else if (action === "backoff_60s") {
        expect(r.state().status).toBe("reconnecting");
        await settle(BACKOFF_LONG_MS - 1);
        expect(r.opens()).toBe(before);
        await settle(1);
        expect(r.opens()).toBe(before + 1);
        expect(r.state().status).toBe("ready");
      } else {
        // reconnect_backoff and reconnect_new_handshake: first delay 375 ms with random 0.5.
        expect(r.state().status).toBe("reconnecting");
        await settle(374);
        expect(r.opens()).toBe(before);
        await settle(1);
        expect(r.opens()).toBe(before + 1);
        expect(r.state().status).toBe("ready");
      }
    });
  }

  it("a BYE after ready reconnects, and a BYE before any state then blocks as revoked", async () => {
    const r = await ready();
    r.desktop.revokeDevice(fingerprint(r.deviceKey.publicKey));
    await settle(2000);
    expect(r.opens()).toBe(2);
    expect(r.state()).toMatchObject({ status: "closed", blocked: "revoked", revoked: true });
  });
});

describe("backoff", () => {
  it("min(30 s, 0.5 s * 2^n) * random(0.5..1)", () => {
    expect(backoffDelay(0, () => 1)).toBe(500);
    expect(backoffDelay(1, () => 1)).toBe(1000);
    expect(backoffDelay(5, () => 1)).toBe(16_000);
    expect(backoffDelay(6, () => 1)).toBe(30_000);
    expect(backoffDelay(20, () => 1)).toBe(30_000);
    expect(backoffDelay(3, () => 0)).toBe(2000);
    expect(backoffDelay(3, () => 0.5)).toBe(3000);
  });

  it("grows while the link keeps failing and is reset after 10 s of ready", async () => {
    const attempts: number[] = [];
    let fail = true;
    const r = rig({
      random: () => 1,
      deps: {
        openRelay: async () => {
          attempts.push(Date.now());
          if (fail) throw new RelayConnectError("down", 1006);
          return new RelayConnection(r.relay.connectDevice(DEVICE_ID));
        },
      },
    });
    const t0 = Date.now();
    await settle();
    // 500, 1000, 2000, 4000 ms between the attempts.
    await settle(500);
    await settle(1000);
    await settle(2000);
    expect(attempts.map((t) => t - t0)).toEqual([0, 500, 1500, 3500]);
    fail = false;
    await settle(4000);
    expect(r.state().status).toBe("ready");
    // Not yet reset: a drop right away waits 8 s (n = 4 after four failures).
    await settle(9_000);
    fail = true;
    r.relay.revokeDevice(DEVICE_ID, 1000);
    const dropAt = Date.now();
    await settle();
    fail = false;
    const opensBefore = attempts.length;
    await settle(7_999);
    expect(attempts.length).toBe(opensBefore);
    await settle(1);
    expect(attempts.length).toBe(opensBefore + 1);
    expect(Date.now() - dropAt).toBe(8000);
    // Ready for 10 s resets: the next drop waits 500 ms again.
    await settle(10_000);
    r.relay.revokeDevice(DEVICE_ID, 1000);
    const n = attempts.length;
    await settle(499);
    expect(attempts.length).toBe(n);
    await settle(1);
    expect(attempts.length).toBe(n + 1);
  });
});

describe("visibility and network", () => {
  it("does not reconnect while hidden and reconnects on the next visible", async () => {
    const r = await ready();
    setHidden(true);
    expect(r.state().status).toBe("ready");
    r.relay.revokeDevice(DEVICE_ID, 1000);
    await settle(120_000);
    expect(r.opens()).toBe(1);
    expect(r.state().status).toBe("reconnecting");
    setHidden(false);
    await settle();
    expect(r.opens()).toBe(2);
    expect(r.state().status).toBe("ready");
  });

  it("starting while hidden waits for visible", async () => {
    doc.hidden = true;
    const r = rig();
    await settle(60_000);
    expect(r.fetchCalls()).toBe(0);
    setHidden(false);
    await settle();
    expect(r.state().status).toBe("ready");
  });

  it("offline pauses attempts until the online event", async () => {
    const r = await ready();
    navigatorState.onLine = false;
    r.relay.revokeDevice(DEVICE_ID, 1000);
    await settle(120_000);
    expect(r.opens()).toBe(1);
    navigatorState.onLine = true;
    win.dispatchEvent(new Event("online"));
    await settle();
    expect(r.opens()).toBe(2);
    expect(r.state().status).toBe("ready");
  });

  it("visible after more than 75 s of silence reconnects, reopens watches with fresh params and emits resynced", async () => {
    const r = await ready();
    const frames: unknown[] = [];
    let from = 0;
    const params: Record<string, unknown>[] = [];
    r.connection.registerWatch({
      method: "conversation.watch",
      reopenParams: () => {
        const value = { conversationId: "sess-local-approval", ...(from > 0 ? { from } : {}) };
        params.push(value);
        return value;
      },
      onFrame: (frame) => frames.push(frame),
    });
    await settle();
    expect(params).toHaveLength(1);
    expect(r.state().watches).toEqual([{ id: 1, method: "conversation.watch" }]);

    // A tab that comes back quickly keeps its socket.
    r.clock.now += 10_000;
    setHidden(true);
    setHidden(false);
    await settle();
    expect(r.opens()).toBe(1);
    expect(r.resynced()).toBe(0);

    from = 12;
    r.clock.now += 80_000;
    setHidden(true);
    setHidden(false);
    await settle();
    expect(r.opens()).toBe(2);
    expect(r.state().status).toBe("ready");
    expect(params).toHaveLength(2);
    expect(params[1]).toMatchObject({ from: 12 });
    expect(r.resynced()).toBe(1);
    expect(frames.length).toBeGreaterThanOrEqual(1);
  });

  it("a dead watch only reopens while the session is alive, and reconnects when it is silent too", async () => {
    const r = await ready({ deps: { sessionOptions: { deadAfterMs: 1000 } } });
    let opened = 0;
    let dead = 0;
    r.connection.registerWatch({
      method: "conversation.watch",
      reopenParams: () => {
        opened += 1;
        return { conversationId: "sess-local-approval" };
      },
      onFrame: () => undefined,
      onDead: () => {
        dead += 1;
      },
    });
    await settle();
    expect(opened).toBe(1);
    // The watch is silent for 1 s but the session clock says it spoke recently: reopen on the same socket.
    await settle(1500);
    expect(dead).toBe(1);
    expect(opened).toBe(2);
    expect(r.opens()).toBe(1);
    // Now the whole session is silent (clock moves on): the next dead watch reconnects.
    r.clock.now += 80_000;
    await settle(1500);
    expect(r.opens()).toBe(2);
    expect(r.state().status).toBe("ready");
  });

  it("unregistering a watch stops it and drops it from the store", async () => {
    const r = await ready();
    const handle = r.connection.registerWatch({ method: "conversation.watch", reopenParams: () => ({ conversationId: "sess-local-approval" }), onFrame: () => undefined });
    await settle();
    handle.unregister();
    expect(r.state().watches).toEqual([]);
    r.clock.now += 80_000;
    setHidden(false);
    setHidden(true);
    setHidden(false);
    await settle();
    expect(r.state().status).toBe("ready");
  });
});

describe("pending, requests and reconnects", () => {
  it("pending survives a reconnect and is never shown as ready", async () => {
    const r = rig({ device: "pending" });
    await settle();
    expect(r.state().status).toBe("pending");
    r.relay.revokeDevice(DEVICE_ID, 1000);
    await settle();
    expect(r.state().status).toBe("reconnecting");
    await settle(1000);
    expect(r.state().status).toBe("pending");
    expect(r.statuses).not.toContain("ready");
    await expect(r.connection.request("workspace.list")).rejects.toMatchObject({ code: "not_ready" });
    r.desktop.confirmDevice(fingerprint(r.deviceKey.publicKey), "control");
    await settle();
    expect(r.state().status).toBe("ready");
  });

  it("requests reject not_ready before ready and are not queued", async () => {
    const spy = vi.spyOn(LinkSession.prototype, "request");
    const r = rig();
    await expect(r.connection.request("workspace.list")).rejects.toMatchObject({ name: "LinkRequestError", code: "not_ready" });
    await settle();
    expect(spy).not.toHaveBeenCalled();
    expect(r.state().status).toBe("ready");
    await expect(r.connection.request("workspace.list")).resolves.toBeTruthy();
    expect(spy).toHaveBeenCalledTimes(1);
  });

  it("a request in flight during a drop rejects and is never re-sent", async () => {
    const spy = vi.spyOn(LinkSession.prototype, "request");
    const r = await ready();
    const pending = r.connection.request("workspace.list");
    const outcome = pending.then(
      () => "resolved",
      (error: { code?: string }) => error.code,
    );
    r.relay.revokeDevice(DEVICE_ID, 1000);
    expect(await outcome).toBe("session_ended");
    await settle(2000);
    expect(r.state().status).toBe("ready");
    expect(spy).toHaveBeenCalledTimes(1);
  });

  it("a reconnect is a fresh Noise session and emits resynced once ready", async () => {
    const r = await ready();
    expect(r.resynced()).toBe(0);
    r.relay.revokeDevice(DEVICE_ID, 1000);
    await settle(1000);
    expect(r.state().status).toBe("ready");
    expect(r.resynced()).toBe(1);
    expect(r.opens()).toBe(2);
  });

  it("reconnect() clears a block and tries again", async () => {
    const r = await ready();
    r.relay.revokeDevice(DEVICE_ID, CloseCodes.REVOKED);
    await settle();
    expect(r.state().blocked).toBe("revoked");
    r.connection.reconnect();
    await settle();
    expect(r.state()).toMatchObject({ status: "ready", blocked: null, revoked: false });
  });

  it("retain and release share one start and stop", async () => {
    const r = rig({ start: false });
    r.connection.retain();
    r.connection.retain();
    await settle();
    expect(r.opens()).toBe(1);
    r.connection.release();
    expect(r.state().status).toBe("ready");
    r.connection.release();
    expect(r.state().status).toBe("idle");
  });
});
