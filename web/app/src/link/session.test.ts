// @vitest-environment node
import { afterEach, describe, expect, it, vi } from "vitest";
import { FrameType, MAX_REQUEST_BYTES, REQUEST_TIMEOUT_BY_METHOD, REQUEST_TIMEOUT_MS, encodeFrame } from "@monoptah/protocol";
import { FIXTURE_HANDLERS, InMemoryRelay, LARGE_FILE_PATH, MockDesktop, fingerprint, generateKeyPair, type FrameSocket, type KeyPair } from "@monoptah/mock-desktop";
import { RelayConnection } from "./relay.ts";
import {
  LinkRequestError,
  LinkSession,
  LinkSessionError,
  type LinkEndDetail,
  type LinkEndReason,
  type LinkSessionOptions,
  type WatchHandle,
} from "./session.ts";

const DESKTOP_ID = "11111111-1111-4111-8111-111111111111";
const DEVICE_ID = "22222222-2222-4222-8222-222222222222";

afterEach(() => {
  vi.useRealTimers();
});

/** Wraps a device socket so a test can hold back what the device sends (a desktop that does not answer yet). Order is kept: Noise nonces need it. */
class MutableSocket implements FrameSocket {
  hold = false;
  readonly #queue: Uint8Array[] = [];
  readonly #inner: FrameSocket;
  constructor(inner: FrameSocket) {
    this.#inner = inner;
  }
  send(bytes: Uint8Array): void {
    if (this.hold) this.#queue.push(bytes);
    else this.#inner.send(bytes);
  }
  release(): void {
    this.hold = false;
    for (const bytes of this.#queue.splice(0)) this.#inner.send(bytes);
  }
  sendText(text: string): void {
    this.#inner.sendText(text);
  }
  close(code?: number): void {
    this.#inner.close(code);
  }
  get onmessage() {
    return this.#inner.onmessage;
  }
  set onmessage(v) {
    this.#inner.onmessage = v;
  }
  get ontext() {
    return this.#inner.ontext;
  }
  set ontext(v) {
    this.#inner.ontext = v;
  }
  get onclose() {
    return this.#inner.onclose;
  }
  set onclose(v) {
    this.#inner.onclose = v;
  }
  get closed() {
    return this.#inner.closed;
  }
}

interface Rig {
  relay: InMemoryRelay;
  desktop: MockDesktop;
  desktopSocket: FrameSocket;
  socket: MutableSocket;
  connection: RelayConnection;
  deviceKey: KeyPair;
  ends: { reason: LinkEndReason; detail: LinkEndDetail }[];
  relayErrors: string[];
  presence: string[];
  states: string[];
  session: Promise<LinkSession>;
}

interface RigOptions {
  device?: "confirmed" | "pending" | "revoked" | "unknown";
  permission?: "read" | "control";
  desktopOnline?: boolean;
  desktop?: ConstructorParameters<typeof MockDesktop>[0] extends infer T ? Partial<T> : never;
  session?: Partial<LinkSessionOptions>;
  /** Hold every device frame back from the start (the handshake never reaches the desktop). */
  hold?: boolean;
}

/** Builds the desktop and starts `LinkSession.connect` (not awaited, so a test can fake timers first). */
function rig(options: RigOptions = {}): Rig {
  const relay = new InMemoryRelay();
  const deviceKey = generateKeyPair();
  const desktop = new MockDesktop({ desktopId: DESKTOP_ID, ...options.desktop });
  const status = options.device ?? "confirmed";
  if (status !== "unknown") desktop.setDevice(fingerprint(deviceKey.publicKey), { status, permission: options.permission ?? "control" });
  const desktopSocket = options.desktopOnline === false ? (null as unknown as FrameSocket) : relay.connectDesktop();
  if (options.desktopOnline !== false) desktop.attach(desktopSocket);
  const socket = new MutableSocket(relay.connectDevice(DEVICE_ID));
  socket.hold = options.hold ?? false;
  const connection = new RelayConnection(socket);
  const ends: Rig["ends"] = [];
  const relayErrors: string[] = [];
  const presence: string[] = [];
  const states: string[] = [];
  const session = LinkSession.connect({
    relay: connection,
    desktopId: DESKTOP_ID,
    deviceId: DEVICE_ID,
    desktopPublicKey: desktop.publicKey,
    deviceKey,
    deviceName: "Test browser",
    userAgent: "vitest",
    onEnd: (reason, detail) => ends.push({ reason, detail }),
    onRelayError: (code) => relayErrors.push(code),
    onPresence: (p) => presence.push(p),
    onState: (s) => states.push(s.state),
    ...options.session,
  });
  return { relay, desktop, desktopSocket, socket, connection, deviceKey, ends, relayErrors, presence, states, session };
}

async function connected(options: RigOptions = {}): Promise<Rig & { s: LinkSession }> {
  const r = rig(options);
  const s = await r.session;
  return { ...r, s };
}

describe("handshake and state", () => {
  it("completes the handshake, gets a confirmed state with capabilities and runs a request", async () => {
    const r = await connected({ desktop: { name: "Example Desktop", version: "9.9.9" } });
    expect(r.s.state).toMatchObject({ type: "state", state: "confirmed", permission: "control", proto: 1, desktop: { name: "Example Desktop", version: "9.9.9" } });
    expect(r.s.state.capabilities).toEqual(expect.arrayContaining(["cloud", "approvals"]));
    expect(r.states).toEqual(["confirmed"]);
    expect(r.s.ready).toBe(true);
    expect(r.desktop.sessions[0]).toMatchObject({ deviceId: DEVICE_ID, channel: 1, state: "confirmed", deviceName: "Test browser" });
    const list = await r.s.request("workspace.list");
    expect(list.projects).toHaveLength(2);
    expect(r.ends).toEqual([]);
  });

  it("a pending device gets state pending and requests reject not_ready", async () => {
    const r = await connected({ device: "pending" });
    expect(r.s.state.state).toBe("pending");
    expect(r.s.ready).toBe(false);
    await expect(r.s.request("workspace.list")).rejects.toMatchObject({ name: "LinkRequestError", code: "not_ready" });
    // The user confirms on the desktop: a new state arrives and requests work.
    r.desktop.confirmDevice(fingerprint(r.deviceKey.publicKey), "control");
    await r.relay.settled();
    expect(r.s.state.state).toBe("confirmed");
    expect(r.states).toEqual(["pending", "confirmed"]);
    expect((await r.s.request("workspace.list")).projects).toHaveLength(2);
  });

  it("a revoked device gets BYE and no state", async () => {
    const r = rig({ device: "revoked" });
    const error = await r.session.catch((e: unknown) => e);
    expect(error).toBeInstanceOf(LinkSessionError);
    expect(error).toMatchObject({ reason: "bye" });
    expect(r.states).toEqual([]);
    expect(r.connection.closed).toBe(true);
  });

  it("a wrong pinned desktop key fails with bye", async () => {
    const wrong = generateKeyPair();
    const r = rig({ session: { desktopPublicKey: wrong.publicKey } });
    await expect(r.session).rejects.toMatchObject({ reason: "bye" });
  });

  it("a re-sent state message updates the session state", async () => {
    const r = await connected();
    r.desktop.setDevice(fingerprint(r.deviceKey.publicKey), { permission: "read" });
    await r.relay.settled();
    expect(r.s.state.permission).toBe("read");
    await expect(r.s.request("conversation.send", { conversationId: "sess-local-1", text: "hi" })).rejects.toMatchObject({
      name: "LinkRequestError",
      code: "read_only",
    });
    r.desktop.setCapabilities(["approvals"]);
    await r.relay.settled();
    expect(r.s.state.capabilities).toEqual(["approvals"]);
    expect(r.states).toEqual(["confirmed", "confirmed", "confirmed"]);
  });

  it("no state within the ready timeout rejects connect with handshake_timeout", async () => {
    vi.useFakeTimers();
    const r = rig({ session: { readyTimeoutMs: 5_000 }, hold: true });
    const settled = r.session.catch((e: unknown) => e);
    await vi.advanceTimersByTimeAsync(5_001);
    expect(await settled).toMatchObject({ reason: "handshake_timeout" });
    expect(r.connection.closed).toBe(true);
  });
});

describe("presence", () => {
  it("reaches the desktop, is cleared with null and does not disturb requests", async () => {
    const r = await connected();
    expect(r.s.sendPresence({ kind: "mono", id: "m1" })).toBe(true);
    await r.relay.settled();
    expect(r.desktop.sessions[0]?.presence).toEqual({ kind: "mono", id: "m1" });
    r.s.sendPresence(null);
    await r.relay.settled();
    expect(r.desktop.sessions[0]?.presence).toBeNull();
    expect((await r.s.request("workspace.list")).projects).toHaveLength(2);
  });

  it("is not sent before the device is confirmed", async () => {
    const r = await connected({ device: "pending" });
    expect(r.s.sendPresence({ kind: "session", id: "s1" })).toBe(false);
  });
});

describe("requests", () => {
  it("surfaces a desktop error code as LinkRequestError", async () => {
    const r = await connected({ permission: "read" });
    const error = await r.s.request("conversation.send", { conversationId: "sess-local-1", text: "hi" }).catch((e: unknown) => e);
    expect(error).toBeInstanceOf(LinkRequestError);
    expect(error).toMatchObject({ code: "read_only", message: "This device can only read" });
  });

  it("times out after 35 seconds with code timeout, and ignores a late answer", async () => {
    vi.useFakeTimers();
    const r = await connected();
    r.socket.hold = true;
    const promise = r.s.request("workspace.list").catch((e: unknown) => e);
    await vi.advanceTimersByTimeAsync(34_999);
    let done = false;
    void promise.then(() => (done = true));
    await Promise.resolve();
    expect(done).toBe(false);
    await vi.advanceTimersByTimeAsync(1);
    expect(await promise).toMatchObject({ name: "LinkRequestError", code: "timeout" });
    // The held request reaches the desktop now; its answer is late and ignored, the next request works.
    r.socket.release();
    expect((await r.s.request("workspace.list")).projects).toHaveLength(2);
    expect(r.s.ended).toBe(false);
  });

  it("rejects with aborted when the signal fires", async () => {
    const r = await connected();
    r.socket.hold = true;
    const controller = new AbortController();
    const promise = r.s.request("workspace.list", {}, { signal: controller.signal });
    controller.abort();
    await expect(promise).rejects.toMatchObject({ code: "aborted" });
  });

  it("refuses an oversized request before sending, and a bad command id", async () => {
    const r = await connected();
    const before = r.relay.forwarded.length;
    await expect(
      r.s.request("conversation.send", { conversationId: "sess-local-1", text: "x".repeat(MAX_REQUEST_BYTES + 1) }),
    ).rejects.toMatchObject({ code: "too_large" });
    await expect(r.s.request("workspace.list", {}, { id: "x".repeat(129) })).rejects.toMatchObject({ code: "invalid_id" });
    await expect(r.s.request("workspace.list", {}, { id: "" })).rejects.toMatchObject({ code: "invalid_id" });
    expect(r.relay.forwarded.length).toBe(before);
    expect((await r.s.request("workspace.list", {}, { id: "x".repeat(128) })).projects).toHaveLength(2);
  });

  it("answers concurrent requests by rid", async () => {
    const r = await connected();
    const [a, b, c] = await Promise.all([r.s.request("workspace.list"), r.s.request("models.list"), r.s.request("workspace.list")]);
    expect(a.projects).toHaveLength(2);
    expect(b).toBeTruthy();
    expect(c.projects).toHaveLength(2);
  });
});

describe("chunking and rekey", () => {
  it("reassembles a chunked large response", async () => {
    const r = await connected();
    const file = await r.s.request("project.file", { projectId: "proj-alpha", path: LARGE_FILE_PATH });
    expect(file.text.length).toBe(200_000);
  });

  it("chunks a large request", async () => {
    const r = await connected();
    const before = r.relay.forwarded.length;
    await r.s.request("conversation.send", { conversationId: "sess-local-1", text: "x".repeat(100_000) });
    expect(r.relay.forwarded.slice(before).filter((f) => f.direction === "device->desktop")).toHaveLength(2);
  });

  it("rekeys on both sides after the message threshold and keeps working, also mid-chunked response", async () => {
    const r = await connected({ desktop: { rekeyAfterMessages: 3 }, session: { rekeyAfterMessages: 4 } });
    for (let i = 0; i < 12; i++) expect((await r.s.request("workspace.list")).projects).toHaveLength(2);
    expect(r.s.rekeysSent).toBeGreaterThan(0);
    expect(r.desktop.sessions[0]?.rekeysReceived).toBe(r.s.rekeysSent);
    expect(r.s.rekeysReceived).toBe(r.desktop.sessions[0]?.rekeysSent);
    expect((await r.s.request("project.file", { projectId: "proj-alpha", path: LARGE_FILE_PATH })).text.length).toBe(200_000);
  });

  it("rekeys after the time threshold using the injected clock", async () => {
    let clock = 1_000_000;
    const r = await connected({ session: { now: () => clock, rekeyAfterMs: 60_000 } });
    await r.s.request("models.list");
    expect(r.s.rekeysSent).toBe(0);
    clock += 60_001;
    await r.s.request("models.list");
    expect(r.s.rekeysSent).toBe(1);
    expect(r.desktop.sessions[0]?.rekeysReceived).toBe(1);
  });
});

describe("watches", () => {
  async function openWatch(r: Rig & { s: LinkSession }, frames: unknown[] = [], extra: Parameters<LinkSession["watch"]>[3] = {}): Promise<WatchHandle> {
    return r.s.watch("conversation.watch", { conversationId: "sess-local-approval" }, (f) => frames.push(f), extra);
  }

  it("opens a watch, delivers frames and stops with cancel", async () => {
    const r = await connected();
    const frames: unknown[] = [];
    const handle = await openWatch(r, frames);
    await r.relay.settled();
    expect(frames.length).toBeGreaterThan(0);
    expect(handle.closed).toBe(false);
    const seen = frames.length;
    expect(r.desktop.advance("sess-local-approval")).toBe(true);
    await r.relay.settled();
    expect(frames.length).toBeGreaterThan(seen);
    handle.stop();
    await r.relay.settled();
    expect(handle.closed).toBe(true);
    const after = frames.length;
    r.desktop.advance("sess-local-approval");
    await r.relay.settled();
    expect(frames.length).toBe(after);
  });

  it("cancel frees the desktop watch slot (4 per device)", async () => {
    const r = await connected();
    const handles: WatchHandle[] = [];
    for (let i = 0; i < 4; i++) handles.push(await openWatch(r));
    await expect(openWatch(r)).rejects.toMatchObject({ code: "busy" });
    handles[0]?.stop();
    await r.relay.settled();
    const again = await openWatch(r);
    expect(again.closed).toBe(false);
  });

  it("an error response to the watch request rejects it", async () => {
    const r = await connected();
    await expect(r.s.watch("conversation.watch", { conversationId: "nope" }, () => undefined)).rejects.toBeInstanceOf(LinkRequestError);
  });

  it("file.prepare waits for the desktop's upload (5 minutes 45 seconds) while other requests time out at 35 seconds", async () => {
    vi.useFakeTimers();
    const original = FIXTURE_HANDLERS["file.prepare"];
    FIXTURE_HANDLERS["file.prepare"] = (() => new Promise(() => undefined)) as never;
    try {
      const r = await connected();
      const prepare = r.s.request("file.prepare", { path: "/tmp/outside-note.txt" }).then(
        () => "answered",
        (e: unknown) => e,
      );
      let settled = false;
      void prepare.then(() => (settled = true));
      await vi.advanceTimersByTimeAsync(REQUEST_TIMEOUT_MS + 1_000);
      expect(settled).toBe(false);
      await vi.advanceTimersByTimeAsync(REQUEST_TIMEOUT_BY_METHOD["file.prepare"]! - REQUEST_TIMEOUT_MS);
      expect(await prepare).toMatchObject({ code: "timeout" });
      // 15 seconds more than the desktop host's 5.5 minutes: its answer, even a busy one, comes first.
      expect(REQUEST_TIMEOUT_BY_METHOD["file.prepare"]).toBe(5 * 60_000 + 30_000 + 15_000);
    } finally {
      FIXTURE_HANDLERS["file.prepare"] = original;
    }
  });

  it("heartbeats keep the watch alive", async () => {
    vi.useFakeTimers();
    const r = await connected();
    let dead = 0;
    const handle = await openWatch(r, [], { onDead: () => (dead += 1) });
    await vi.advanceTimersByTimeAsync(300_000);
    expect(dead).toBe(0);
    expect(handle.closed).toBe(false);
  });

  it("75 seconds with no frame and no heartbeat marks the watch dead", async () => {
    vi.useFakeTimers();
    const r = await connected({ desktop: { heartbeatMs: 10_000_000 } });
    let dead = 0;
    const handle = await openWatch(r, [], { onDead: () => (dead += 1) });
    await vi.advanceTimersByTimeAsync(74_000);
    expect(dead).toBe(0);
    await vi.advanceTimersByTimeAsync(1_001);
    expect(dead).toBe(1);
    expect(handle.closed).toBe(true);
    // The session itself is still usable.
    expect((await r.s.request("workspace.list")).projects).toHaveLength(2);
  });

  it("a session end closes open watches with session_ended", async () => {
    const r = await connected();
    const errors: LinkRequestError[] = [];
    const handle = await openWatch(r, [], { onError: (e) => errors.push(e) });
    r.s.close();
    expect(handle.closed).toBe(true);
    expect(errors[0]).toMatchObject({ code: "session_ended" });
  });
});

describe("ending the session", () => {
  it("BYE from the desktop ends the session and rejects pending requests with session_ended", async () => {
    const r = await connected();
    const pending = r.s.request("workspace.list").catch((e: unknown) => e);
    r.desktop.revokeDevice(fingerprint(r.deviceKey.publicKey));
    expect(await pending).toMatchObject({ code: "session_ended" });
    await r.relay.settled();
    expect(r.ends).toEqual([{ reason: "bye", detail: {} }]);
    expect(r.s.ended).toBe(true);
    await expect(r.s.request("workspace.list")).rejects.toMatchObject({ code: "session_ended" });
    expect(r.connection.closed).toBe(true);
  });

  it("a TRANSPORT body that does not decrypt ends the session and sends BYE", async () => {
    const r = await connected();
    const pending = r.s.request("workspace.list", {}).catch((e: unknown) => e);
    r.desktopSocket.send(encodeFrame({ type: FrameType.TRANSPORT, channel: 1, deviceId: DEVICE_ID, body: new Uint8Array(40).fill(7) }));
    await r.relay.settled();
    expect(await pending).toMatchObject({ code: "session_ended" });
    expect(r.ends.map((e) => e.reason)).toEqual(["decrypt_failed"]);
  });

  it("a relay close ends the session with the close code", async () => {
    const r = await connected();
    r.relay.revokeDevice(DEVICE_ID, 4003);
    await r.relay.settled();
    expect(r.ends).toEqual([{ reason: "relay_closed", detail: { closeCode: 4003, closeReason: "" } }]);
  });

  it("close() sends BYE, reports closed once and the desktop drops the session", async () => {
    // The in-memory relay drops frames sent just before a close, unlike a WebSocket, so keep the socket open here.
    const r = await connected({ session: { closeRelayOnEnd: false } });
    r.s.close();
    r.s.close();
    await r.relay.settled();
    expect(r.ends.map((e) => e.reason)).toEqual(["closed"]);
    expect(r.desktop.sessions).toHaveLength(0);
  });

  it("frames for another channel are ignored", async () => {
    const r = await connected();
    r.desktopSocket.send(encodeFrame({ type: FrameType.BYE, channel: 99, deviceId: DEVICE_ID }));
    await r.relay.settled();
    expect(r.s.ended).toBe(false);
    expect((await r.s.request("workspace.list")).projects).toHaveLength(2);
  });
});

describe("relay signals", () => {
  it("surfaces ERROR desktop_offline during the handshake and rejects connect", async () => {
    const r = rig({ desktopOnline: false });
    await expect(r.session).rejects.toMatchObject({ reason: "relay_error", detail: { relayCode: "desktop_offline" } });
    expect(r.relayErrors).toEqual(["desktop_offline"]);
    expect(r.presence).toEqual(["offline"]);
  });

  it("surfaces presence CTRL and a later ERROR desktop_offline after the session is ready", async () => {
    const r = await connected();
    expect(r.presence).toEqual(["online"]);
    r.desktop.close();
    await r.relay.settled();
    expect(r.presence).toEqual(["online", "offline"]);
    // The device frame now bounces off the relay with desktop_offline; the request later times out.
    vi.useFakeTimers();
    const pending = r.s.request("workspace.list").catch((e: unknown) => e);
    await vi.advanceTimersByTimeAsync(0);
    await r.relay.settled();
    expect(r.relayErrors).toEqual(["desktop_offline"]);
    await vi.advanceTimersByTimeAsync(35_001);
    expect(await pending).toMatchObject({ code: "timeout" });
    expect(r.s.ended).toBe(false);
  });

  it("lastFrameAt follows every received frame with the injected clock", async () => {
    let clock = 1_000;
    const r = await connected({ session: { now: () => clock } });
    const first = r.s.lastFrameAt;
    clock = 5_000;
    await r.s.request("workspace.list");
    expect(r.s.lastFrameAt).toBe(5_000);
    expect(first).toBeLessThanOrEqual(1_000);
  });

  it("lastFrameAt ignores CTRL frames and other channels, and onActivity follows the accepted frames (M1, L10)", async () => {
    let clock = 1_000;
    let activity = 0;
    const r = await connected({ session: { now: () => clock, onActivity: () => void (activity += 1) } });
    const base = activity;
    // Another tab's channel on the same socket.
    clock = 9_000;
    r.socket.onmessage?.(encodeFrame({ type: FrameType.BYE, channel: 99, deviceId: DEVICE_ID }));
    expect(r.s.lastFrameAt).toBeLessThanOrEqual(1_000);
    // A presence CTRL frame.
    r.desktop.close();
    await r.relay.settled();
    expect(r.presence).toContain("offline");
    expect(r.s.lastFrameAt).toBeLessThanOrEqual(1_000);
    expect(activity).toBe(base);
  });

  it("the session owns and zeroes the device private key once the handshake is over (L5)", async () => {
    const key = generateKeyPair();
    const copy = { privateKey: key.privateKey.slice(), publicKey: key.publicKey.slice() };
    const relay = new InMemoryRelay();
    const desktop = new MockDesktop({ desktopId: DESKTOP_ID });
    desktop.setDevice(fingerprint(key.publicKey), { status: "confirmed", permission: "control" });
    desktop.attach(relay.connectDesktop());
    const session = await LinkSession.connect({
      relay: new RelayConnection(relay.connectDevice(DEVICE_ID)),
      desktopId: DESKTOP_ID,
      deviceId: DEVICE_ID,
      desktopPublicKey: desktop.publicKey,
      deviceKey: copy,
      deviceName: "Test browser",
      userAgent: "vitest",
    });
    expect(session.ready).toBe(true);
    expect(copy.privateKey.every((byte) => byte === 0)).toBe(true);
    expect((await session.request("workspace.list")).projects).toHaveLength(2);
  });
});
