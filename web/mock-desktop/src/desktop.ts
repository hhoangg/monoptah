// MockDesktop: the desktop side (Noise responder) of the web link, with canned data.
// It behaves as architecture.md 3.3, 5.1 to 5.5 and 7.1 describe, over any `FrameSocket`
// (the in-memory relay of transport.ts, or a real relay through `connectRelay`).
import {
  DeviceMessageSchema,
  FrameType,
  HEARTBEAT_INTERVAL_MS,
  HOST_MAX_WATCHES_PER_DEVICE,
  KNOWN_CAPABILITIES,
  METHODS,
  PENDING_CONFIRMATION_TIMEOUT_MS,
  PROTOCOL_VERSION,
  PlaintextError,
  ReassemblyError,
  SplitError,
  decodeEnvelopeJson,
  decodeFrame,
  encodeEnvelope,
  encodeFrame,
  classify,
  type ClassifyFlags,
  type DesktopErrorCode,
  type EnvelopeRequest,
  type EnvelopeResponse,
  type Frame,
  type MethodName,
  type PresenceTarget,
  type StateMessage,
} from "@monoptah/protocol";
import { HandshakeState, NoiseError, fingerprint, generateKeyPair, prologue, type KeyPair } from "./noise.ts";
import { SecureChannel } from "./secure-channel.ts";
import { MockTransfers } from "./transfer.ts";
import { MockTerminals, type MockTerminalOptions } from "./terminal.ts";
import { FIXTURE_HANDLERS, FixtureError, MockWorld, type HandlerContext, type MockDevice, type WatchCursor } from "./fixtures.ts";
import type { FrameSocket } from "./transport.ts";

export interface Timers {
  setInterval(fn: () => void, ms: number): unknown;
  clearInterval(handle: unknown): void;
  setTimeout(fn: () => void, ms: number): unknown;
  clearTimeout(handle: unknown): void;
}

/** Looks the globals up at call time, so `vi.useFakeTimers()` works whenever it is installed. */
export const globalTimers: Timers = {
  setInterval: (fn, ms) => globalThis.setInterval(fn, ms),
  clearInterval: (handle) => globalThis.clearInterval(handle as ReturnType<typeof setInterval>),
  setTimeout: (fn, ms) => globalThis.setTimeout(fn, ms),
  clearTimeout: (handle) => globalThis.clearTimeout(handle as ReturnType<typeof setTimeout>),
};

export interface MockDesktopOptions {
  /** UUID of the desktop, part of the prologue. */
  desktopId: string;
  /** Desktop static key; default: a fresh random pair. */
  staticKey?: KeyPair;
  name?: string;
  /** Version shown in `state.desktop.version`. */
  version?: string;
  /** Capability names advertised in `state` (default: every known one except `terminal`, which a test turns on: a shell is opt-in). */
  capabilities?: string[];
  /** Devices that are known up front. An unknown device becomes `pending`. */
  devices?: (Partial<Omit<MockDevice, "fingerprint">> & { fingerprint: string })[];
  /** Desktop toggles that open denied paths ("Remote push in Git", "Remote cloud start"). Default off. */
  flags?: ClassifyFlags;
  world?: MockWorld;
  timers?: Timers;
  now?: () => number;
  heartbeatMs?: number;
  /** Rekey after this many sent messages (default 2^16). Lower it to test rekeying. */
  rekeyAfterMessages?: number;
  rekeyAfterMs?: number;
  /** Largest response the desktop sends (default 8 MiB); a larger one is answered `too_large`. Lower it in tests. */
  maxResponseBytes?: number;
  /** When set, scripted fixture steps (cloud startup, approval clearing) advance on this period. */
  autoAdvanceMs?: number;
  /** Limits and hooks of the fake shells (`terminal.*`); the clock is the desktop's own. */
  terminal?: Omit<MockTerminalOptions, "clock">;
  /** Fixed ephemeral private key for deterministic handshakes. Default: random. */
  ephemeralPrivateKey?: () => Uint8Array;
}

interface Watch {
  conversationId: string;
  cursor: WatchCursor;
  unsubscribe: () => void;
  heartbeat: unknown;
}

interface Session {
  key: string;
  deviceId: string;
  channel: number;
  device: MockDevice;
  /** Untrusted name from the HS1 payload. */
  deviceName: string | undefined;
  channelCrypto: SecureChannel;
  watches: Map<number, Watch>;
  pendingTimeout: unknown;
  /** The last presence report of this session (what the page shows), null when cleared. */
  presence: { target: PresenceTarget; at: number } | null;
}

/** What a test can see of a live session. */
export interface SessionInfo {
  deviceId: string;
  channel: number;
  fingerprint: string;
  deviceName: string | undefined;
  state: MockDevice["status"];
  openWatches: number;
  /** What the page reports it shows (visible and focused), null when nothing. Reports have no answer. */
  presence: PresenceTarget | null;
  rekeysSent: number;
  rekeysReceived: number;
}

/** Capabilities that gate a method family, so a desktop that does not advertise them answers like an older one. */
function requiredCapability(method: MethodName): string | undefined {
  if (method.startsWith("cloud.")) return "cloud";
  if (method === "session.answerApproval") return "approvals";
  if (method === "chat.mentionedFile") return "mentionedFiles";
  if (method === "file.prepare") return "fileTransfer";
  if (method.startsWith("terminal.")) return "terminal";
  return undefined;
}

const textEncoder = new TextEncoder();

export class MockDesktop {
  readonly desktopId: string;
  readonly staticKey: KeyPair;
  readonly fingerprint: string;
  readonly world: MockWorld;
  /** The desktop's encrypted file transfers (`file.prepare`); set `transfers.backend` to a server to enable uploads. */
  readonly transfers = new MockTransfers();
  /** The desktop's fake shells (`terminal.*`); `terminals.verifyTicket` is how the desktop spends a ticket at the Worker (unset: fail closed). */
  readonly terminals: MockTerminals;
  /** Known devices by fingerprint. */
  readonly devices = new Map<string, MockDevice>();
  flags: ClassifyFlags;
  /** Relay CTRL and ERROR frames received (metadata only), for tests. */
  readonly relayFrames: Frame[] = [];

  readonly #name: string;
  readonly #version: string;
  #capabilities: string[];
  readonly #sessions = new Map<string, Session>();
  readonly #timers: Timers;
  readonly #now: () => number;
  readonly #heartbeatMs: number;
  readonly #rekeyAfterMessages: number | undefined;
  readonly #rekeyAfterMs: number | undefined;
  readonly #maxResponseBytes: number | undefined;
  readonly #ephemeral: (() => Uint8Array) | undefined;
  #socket: FrameSocket | null = null;
  #autoAdvance: unknown = null;

  constructor(options: MockDesktopOptions) {
    this.desktopId = options.desktopId;
    this.staticKey = options.staticKey ?? generateKeyPair();
    this.fingerprint = fingerprint(this.staticKey.publicKey);
    this.#name = options.name ?? "Example Desktop";
    this.#version = options.version ?? "0.0.0-mock";
    this.#capabilities = [...(options.capabilities ?? KNOWN_CAPABILITIES.filter((name) => name !== "terminal"))];
    this.flags = options.flags ?? {};
    this.#timers = options.timers ?? globalTimers;
    this.#now = options.now ?? Date.now;
    this.#heartbeatMs = options.heartbeatMs ?? HEARTBEAT_INTERVAL_MS;
    this.#rekeyAfterMessages = options.rekeyAfterMessages;
    this.#rekeyAfterMs = options.rekeyAfterMs;
    this.#maxResponseBytes = options.maxResponseBytes;
    this.#ephemeral = options.ephemeralPrivateKey;
    this.world = options.world ?? new MockWorld({ now: this.#now });
    this.terminals = new MockTerminals({
      ...options.terminal,
      clock: {
        now: () => this.#now(),
        setTimeout: (fn, ms) => this.#timers.setTimeout(fn, ms),
        clearTimeout: (handle) => this.#timers.clearTimeout(handle),
      },
    });
    this.#syncWorldCapabilities();
    for (const device of options.devices ?? []) this.setDevice(device.fingerprint, device);
    if (options.autoAdvanceMs !== undefined) {
      this.#autoAdvance = this.#timers.setInterval(() => this.world.advanceAll(), options.autoAdvanceMs);
    }
  }

  get publicKey(): Uint8Array {
    return this.staticKey.publicKey;
  }

  get capabilities(): readonly string[] {
    return this.#capabilities;
  }

  // --- Connection ---

  /** Starts serving frames from `socket` (the desktop end of a relay). */
  attach(socket: FrameSocket): void {
    this.#socket = socket;
    socket.onmessage = (bytes) => this.#onFrame(bytes);
    socket.ontext = () => undefined;
    socket.onclose = () => {
      this.#dropAllSessions();
      this.#socket = null;
    };
  }

  /** Stops timers and drops every session. */
  close(): void {
    if (this.#autoAdvance !== null) this.#timers.clearInterval(this.#autoAdvance);
    this.#autoAdvance = null;
    this.#dropAllSessions();
    this.#socket?.close();
    this.#socket = null;
  }

  // --- Device list and settings ---

  /** Adds or updates a device. Defaults: `pending`, `read`, selection all. */
  setDevice(fingerprintHex: string, patch: Partial<Omit<MockDevice, "fingerprint">> = {}): MockDevice {
    const existing = this.devices.get(fingerprintHex);
    const device: MockDevice = existing ?? { fingerprint: fingerprintHex, status: "pending", permission: "read", selection: { projects: null, monos: null } };
    Object.assign(device, patch);
    this.devices.set(fingerprintHex, device);
    // A device that lost Control or whose "Allow web terminal" went off loses its shells at once.
    if (!this.#terminalAllowed(device)) this.terminals.killDevice(fingerprintHex, "toggle_off");
    this.#resendState(device);
    return device;
  }

  /** The user confirmed the device on the desktop: `state` goes to every live channel of it. */
  confirmDevice(fingerprintHex: string, permission: "read" | "control" = "control"): void {
    this.setDevice(fingerprintHex, { status: "confirmed", permission });
  }

  /** The user revoked the device: BYE on each of its channels, no state. */
  revokeDevice(fingerprintHex: string): void {
    const device = this.setDevice(fingerprintHex, { status: "revoked" });
    this.terminals.killDevice(fingerprintHex, "revoked");
    for (const session of [...this.#sessions.values()]) if (session.device === device) this.#endSession(session, true);
  }

  /** Changes the advertised capabilities; every live confirmed channel of every device gets a new `state` (architecture.md 3.3 step 4). */
  setCapabilities(capabilities: string[]): void {
    this.#capabilities = [...capabilities];
    this.#syncWorldCapabilities();
    if (!this.#capabilities.includes("terminal")) this.terminals.killAll("toggle_off");
    for (const session of this.#sessions.values()) this.#sendState(session);
  }

  #syncWorldCapabilities(): void {
    this.world.capabilities = { cloud: this.#capabilities.includes("cloud"), approvals: this.#capabilities.includes("approvals") };
  }

  /** Runs the next scripted fixture step of a conversation. */
  advance(conversationId: string): boolean {
    return this.world.advance(conversationId);
  }

  get sessions(): SessionInfo[] {
    return [...this.#sessions.values()].map((s) => ({
      deviceId: s.deviceId,
      channel: s.channel,
      fingerprint: s.device.fingerprint,
      deviceName: s.deviceName,
      state: s.device.status,
      openWatches: s.watches.size,
      presence: s.presence?.target ?? null,
      rekeysSent: s.channelCrypto.rekeysSent,
      rekeysReceived: s.channelCrypto.rekeysReceived,
    }));
  }

  // --- Frames ---

  #key(deviceId: string, channel: number): string {
    return `${deviceId}/${channel}`;
  }

  #sendFrame(type: Frame["type"], deviceId: string, channel: number, body?: Uint8Array): void {
    this.#socket?.send(encodeFrame({ type, deviceId, channel, ...(body ? { body } : {}) }));
  }

  #bye(deviceId: string, channel: number): void {
    this.#sendFrame(FrameType.BYE, deviceId, channel);
  }

  #onFrame(bytes: Uint8Array): void {
    let frame: Frame;
    try {
      frame = decodeFrame(bytes);
    } catch {
      return;
    }
    switch (frame.type) {
      case FrameType.HS1:
        this.#onHandshake(frame);
        break;
      case FrameType.TRANSPORT:
        this.#onTransport(frame);
        break;
      case FrameType.BYE: {
        const session = this.#sessions.get(this.#key(frame.deviceId, frame.channel));
        if (session) this.#endSession(session, false);
        break;
      }
      case FrameType.CTRL:
      case FrameType.ERROR:
        this.relayFrames.push(frame);
        break;
      default:
        // HS2 is never sent to a desktop.
        break;
    }
  }

  // --- Handshake (architecture.md 5.3) ---

  #onHandshake(frame: Frame): void {
    const key = this.#key(frame.deviceId, frame.channel);
    const previous = this.#sessions.get(key);
    if (previous) this.#endSession(previous, false);

    const handshake = new HandshakeState({
      initiator: false,
      prologue: prologue(this.desktopId, frame.deviceId, frame.channel),
      s: this.staticKey,
      ...(this.#ephemeral ? { ephemeralPrivateKey: this.#ephemeral() } : {}),
    });
    let payload: Uint8Array;
    try {
      payload = handshake.readMessage(frame.body);
    } catch (error) {
      if (!(error instanceof NoiseError)) throw error;
      this.#bye(frame.deviceId, frame.channel);
      return;
    }
    const remoteStatic = handshake.remoteStatic as Uint8Array;
    const fp = fingerprint(remoteStatic);
    let device = this.devices.get(fp);
    if (device?.status === "revoked") {
      this.#bye(frame.deviceId, frame.channel);
      return;
    }
    if (!device) {
      // Unknown: the handshake completes so the desktop can show the confirmation prompt.
      device = this.setDevice(fp, { status: "pending", permission: "read" });
    }

    const hs2 = handshake.writeMessage(textEncoder.encode(JSON.stringify({ proto: PROTOCOL_VERSION })));
    const { send, recv } = handshake.split();
    const session: Session = {
      key,
      deviceId: frame.deviceId,
      channel: frame.channel,
      device,
      deviceName: parseDeviceName(payload),
      channelCrypto: new SecureChannel({
        send,
        recv,
        now: this.#now,
        ...(this.#rekeyAfterMessages === undefined ? {} : { rekeyAfterMessages: this.#rekeyAfterMessages }),
        ...(this.#rekeyAfterMs === undefined ? {} : { rekeyAfterMs: this.#rekeyAfterMs }),
        ...(this.#maxResponseBytes === undefined ? {} : { maxOutgoingBytes: this.#maxResponseBytes }),
      }),
      watches: new Map(),
      pendingTimeout: null,
      presence: null,
    };
    this.#sessions.set(key, session);
    this.#sendFrame(FrameType.HS2, frame.deviceId, frame.channel, hs2);
    this.#sendState(session);
  }

  // --- State message ---

  /** Control and the per-device toggle (default on in the mock): what the desktop checks before it advertises `terminal`. */
  #terminalAllowed(device: MockDevice): boolean {
    return device.status === "confirmed" && device.permission === "control" && device.terminal !== false;
  }

  #stateMessage(device: MockDevice): StateMessage {
    if (device.status !== "confirmed") return { type: "state", state: device.status };
    const capabilities = this.#terminalAllowed(device) ? [...this.#capabilities] : this.#capabilities.filter((name) => name !== "terminal");
    return {
      type: "state",
      state: "confirmed",
      permission: device.permission,
      proto: PROTOCOL_VERSION,
      capabilities,
      desktop: { name: this.#name, version: this.#version },
    };
  }

  #sendState(session: Session): void {
    if (session.device.status === "revoked") return;
    this.#send(session, this.#stateMessage(session.device));
    this.#armPendingTimeout(session);
  }

  #resendState(device: MockDevice): void {
    for (const session of this.#sessions.values()) if (session.device === device) this.#sendState(session);
  }

  /** A pending device is dropped with BYE after 5 minutes without confirmation (5.3). */
  #armPendingTimeout(session: Session): void {
    if (session.pendingTimeout !== null) this.#timers.clearTimeout(session.pendingTimeout);
    session.pendingTimeout = null;
    if (session.device.status !== "pending") return;
    session.pendingTimeout = this.#timers.setTimeout(() => {
      session.pendingTimeout = null;
      if (this.#sessions.get(session.key) === session && session.device.status === "pending") this.#endSession(session, true);
    }, PENDING_CONFIRMATION_TIMEOUT_MS);
  }

  // --- Transport ---

  #send(session: Session, message: StateMessage | EnvelopeResponse): void {
    let bodies: Uint8Array[];
    try {
      bodies = session.channelCrypto.seal(encodeEnvelope(message));
    } catch (error) {
      const rid = "rid" in message ? message.rid : undefined;
      if (!(error instanceof SplitError) || typeof rid !== "number") throw error;
      bodies = session.channelCrypto.seal(encodeEnvelope({ rid, ok: false, error: "Response is too large", code: "too_large" }));
    }
    for (const body of bodies) this.#sendFrame(FrameType.TRANSPORT, session.deviceId, session.channel, body);
  }

  #onTransport(frame: Frame): void {
    const session = this.#sessions.get(this.#key(frame.deviceId, frame.channel));
    if (!session) {
      // An unknown (deviceId, channel) pair other than HS1: BYE (5.1).
      this.#bye(frame.deviceId, frame.channel);
      return;
    }
    let result;
    try {
      result = session.channelCrypto.open(frame.body);
    } catch (error) {
      if (error instanceof NoiseError || error instanceof PlaintextError || error instanceof ReassemblyError) {
        this.#endSession(session, true);
        return;
      }
      throw error;
    }
    if (result.type !== "message") return;
    this.#onMessage(session, result.bytes);
  }

  #onMessage(session: Session, bytes: Uint8Array): void {
    let json: unknown;
    try {
      json = decodeEnvelopeJson(bytes);
    } catch {
      return;
    }
    if (session.device.status === "revoked") {
      this.#endSession(session, true);
      return;
    }
    // A pending device is ignored until the desktop confirms it (5.3).
    if (session.device.status !== "confirmed") return;
    const parsed = DeviceMessageSchema.safeParse(json);
    if (!parsed.success) {
      const rid = (json as { rid?: unknown } | null)?.rid;
      if (typeof rid === "number" && Number.isInteger(rid) && rid >= 0) this.#send(session, { rid, ok: false, error: "Invalid request" });
      return;
    }
    const message = parsed.data;
    if ("t" in message) {
      session.presence = message.target ? { target: message.target, at: this.#now() } : null;
      return;
    }
    if ("cancel" in message) {
      this.#closeWatch(session, message.rid);
      return;
    }
    this.#onRequest(session, message);
  }

  #error(session: Session, rid: number, error: string, code?: DesktopErrorCode): void {
    this.#send(session, { rid, ok: false, error, ...(code ? { code } : {}) });
  }

  #onRequest(session: Session, request: EnvelopeRequest): void {
    const { rid, method, params } = request;
    const resolved = classify(method, params, this.flags);
    if (resolved === "unknown") return this.#error(session, rid, "Unsupported mobile operation");
    const name = method as MethodName;
    const capability = requiredCapability(name);
    if (capability && !this.#capabilities.includes(capability)) return this.#error(session, rid, "Unsupported mobile operation");
    if (METHODS[name].quicOnly || resolved === "denied") {
      const toggleGated = name === "session.start" && (params as { target?: unknown }).target !== undefined;
      return this.#error(session, rid, "This action is not available from the web", toggleGated ? "disabled" : "forbidden");
    }
    if (resolved === "unsupportedWeb") return this.#error(session, rid, "Not supported on the web", "unsupported");
    if ((resolved === "control" || resolved === "terminal") && session.device.permission === "read") {
      return this.#error(session, rid, "This device can only read", "read_only");
    }
    // `terminal.read` and `terminal.close` stay open so a shell the toggle just killed still delivers its `toggle_off` message.
    if (resolved === "terminal" && session.device.terminal === false && name !== "terminal.read" && name !== "terminal.close") {
      return this.#error(session, rid, "Allow web terminal is off for this device", "disabled");
    }
    const checked = METHODS[name].params.safeParse(params);
    if (!checked.success) return this.#error(session, rid, "Invalid parameters");

    if (name === "conversation.watch") {
      if (request.watch !== true) return this.#error(session, rid, "conversation.watch needs watch: true");
      return this.#openWatch(session, rid, (checked.data as { conversationId: string }).conversationId);
    }
    if (request.watch === true) return this.#error(session, rid, "Only conversation.watch can be watched");

    const ctx: HandlerContext = { world: this.world, device: session.device, transfers: this.transfers, terminals: this.terminals };
    const fail = (error: unknown): void => {
      if (error instanceof FixtureError) return this.#error(session, rid, error.message, error.code);
      throw error;
    };
    try {
      const handler = FIXTURE_HANDLERS[name] as (p: unknown, c: HandlerContext) => unknown;
      const result = handler(checked.data, ctx);
      if (result instanceof Promise) {
        // An upload to the server: the answer comes when it is done. The session may have ended meanwhile.
        void result.then(
          (value) => {
            if (this.#sessions.get(session.key) === session) this.#send(session, { rid, ok: true, result: value });
          },
          (error: unknown) => {
            if (this.#sessions.get(session.key) === session) fail(error);
          },
        );
        return;
      }
      this.#send(session, { rid, ok: true, result });
      if (name === "device.leave") this.#leave(session);
    } catch (error) {
      fail(error);
    }
  }

  /** `device.leave`: the desktop forgets the device and ends its sessions. */
  #leave(session: Session): void {
    this.devices.delete(session.device.fingerprint);
    this.terminals.killDevice(session.device.fingerprint, "revoked");
    for (const other of [...this.#sessions.values()]) if (other.device === session.device) this.#endSession(other, true);
  }

  // --- Watches ---

  #openWatch(session: Session, rid: number, conversationId: string): void {
    if (session.watches.has(rid)) return this.#error(session, rid, "That request id is already watching");
    let open = 0;
    for (const s of this.#sessions.values()) if (s.device === session.device) open += s.watches.size;
    if (open >= HOST_MAX_WATCHES_PER_DEVICE) return this.#error(session, rid, "Too many open watches", "busy");
    try {
      this.world.session(conversationId);
    } catch (error) {
      if (error instanceof FixtureError) return this.#error(session, rid, error.message, error.code);
      throw error;
    }
    const cursor = this.world.newCursor();
    const emit = (): void => {
      const frame = this.world.nextFrame(conversationId, cursor);
      if (frame) this.#send(session, { rid, ok: true, result: frame });
    };
    const watch: Watch = {
      conversationId,
      cursor,
      unsubscribe: this.world.subscribe(conversationId, emit),
      heartbeat: this.#timers.setInterval(() => this.#send(session, { rid, ok: true, heartbeat: true }), this.#heartbeatMs),
    };
    session.watches.set(rid, watch);
    this.#send(session, { rid, ok: true, watching: true });
    emit();
  }

  #closeWatch(session: Session, rid: number): void {
    const watch = session.watches.get(rid);
    if (!watch) return;
    watch.unsubscribe();
    this.#timers.clearInterval(watch.heartbeat);
    session.watches.delete(rid);
  }

  // --- Session end ---

  #endSession(session: Session, sendBye: boolean): void {
    for (const rid of [...session.watches.keys()]) this.#closeWatch(session, rid);
    if (session.pendingTimeout !== null) this.#timers.clearTimeout(session.pendingTimeout);
    session.pendingTimeout = null;
    session.channelCrypto.reset();
    if (this.#sessions.get(session.key) === session) this.#sessions.delete(session.key);
    // The last link of a device is gone: its shells get the grace window, then die.
    if (![...this.#sessions.values()].some((other) => other.device === session.device)) this.terminals.linkLost(session.device.fingerprint);
    if (sendBye) this.#bye(session.deviceId, session.channel);
  }

  #dropAllSessions(): void {
    for (const session of [...this.#sessions.values()]) this.#endSession(session, false);
  }
}

function parseDeviceName(payload: Uint8Array): string | undefined {
  try {
    const value = JSON.parse(new TextDecoder().decode(payload)) as { name?: unknown };
    return typeof value.name === "string" ? value.name.slice(0, 100) : undefined;
  } catch {
    return undefined;
  }
}
