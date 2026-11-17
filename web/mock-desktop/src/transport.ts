// Frame transport: the `FrameSocket` both sides of a test talk through, an in-memory
// relay that applies the DesktopRoom rules of architecture.md 4.4, and a WebSocket
// adapter for later runs against `wrangler dev`.
import {
  CloseCodes,
  FRAME_VERSION,
  FrameType,
  MAX_FRAME_SIZE,
  MIN_FRAME_SIZE,
  ROOM_QUOTAS,
  bytesToUuid,
  uuidToBytes,
  decodeFrame,
  desktopMaySend,
  deviceMaySend,
  encodeCtrlBody,
  encodeErrorBody,
  encodeFrame,
  overwriteDeviceId,
  type CtrlMessage,
  type Frame,
} from "@monoptah/protocol";

/** One end of a relay connection. Binary frames use `send` and `onmessage`; text is only the `ping` keepalive. */
export interface FrameSocket {
  send(bytes: Uint8Array): void;
  sendText(text: string): void;
  /** Closes with a WebSocket close code (default 1000). */
  close(code?: number): void;
  onmessage: ((bytes: Uint8Array) => void) | null;
  ontext: ((text: string) => void) | null;
  onclose: ((code: number) => void) | null;
  readonly closed: boolean;
}

// --- In-memory relay ---

type Role = "desktop" | "device";

class RelayEndpoint implements FrameSocket {
  onmessage: ((bytes: Uint8Array) => void) | null = null;
  ontext: ((text: string) => void) | null = null;
  onclose: ((code: number) => void) | null = null;
  closed = false;
  readonly role: Role;
  /** The authenticated device id of a device socket. */
  readonly deviceId: string | null;
  readonly #relay: InMemoryRelay;

  constructor(relay: InMemoryRelay, role: Role, deviceId: string | null) {
    this.#relay = relay;
    this.role = role;
    this.deviceId = deviceId;
  }

  send(bytes: Uint8Array): void {
    if (this.closed) return;
    const copy = bytes.slice();
    this.#relay._defer(() => this.#relay._fromSocket(this, copy));
  }

  sendText(text: string): void {
    if (this.closed) return;
    this.#relay._defer(() => this.#relay._textFromSocket(this, text));
  }

  close(code = CloseCodes.NORMAL): void {
    if (this.closed) return;
    this.#relay._closeEndpoint(this, code);
  }

  /** Relay to peer, asynchronous like a real socket. */
  _deliver(bytes: Uint8Array): void {
    if (this.closed) return;
    const copy = bytes.slice();
    this.#relay._defer(() => this.onmessage?.(copy));
  }

  _deliverText(text: string): void {
    if (this.closed) return;
    this.#relay._defer(() => this.ontext?.(text));
  }
}

export interface ForwardedFrame {
  direction: "device->desktop" | "desktop->device";
  /** The frame as the receiver sees it. */
  frame: Frame;
}

export interface InMemoryRelayOptions {
  /** Device sockets per device id (default 4, architecture.md 4.4). */
  deviceSocketsPerDevice?: number;
  /** Device sockets per room (default 16). */
  deviceSocketsPerRoom?: number;
}

/**
 * One room of the relay: at most one desktop socket and several device sockets.
 * Rules (architecture.md 4.4): the deviceId of every device -> desktop frame is
 * overwritten with the authenticated id; frames are 20 to 65,555 bytes; a device may
 * send only HS1, TRANSPORT and BYE (anything else is dropped); a frame with no desktop
 * online is answered with an ERROR `desktop_offline`; the only text is `ping`,
 * answered `pong`; presence and device CTRL frames; a new desktop closes the old one
 * with 4009. Delivery is asynchronous (microtasks); `settled()` waits for it.
 * Rate limits are not modelled.
 */
export class InMemoryRelay {
  #desktop: RelayEndpoint | null = null;
  readonly #devices = new Set<RelayEndpoint>();
  readonly #perDevice: number;
  readonly #perRoom: number;
  #pending = 0;

  /** Every frame the relay forwarded, in order (for tests). */
  readonly forwarded: ForwardedFrame[] = [];
  /** Frames the relay dropped because the sender was not allowed to send that type. */
  dropped = 0;

  constructor(options: InMemoryRelayOptions = {}) {
    this.#perDevice = options.deviceSocketsPerDevice ?? ROOM_QUOTAS.deviceSocketsPerDevice;
    this.#perRoom = options.deviceSocketsPerRoom ?? ROOM_QUOTAS.deviceSocketsPerRoom;
  }

  get desktopOnline(): boolean {
    return this.#desktop !== null;
  }

  /** The desktop end. A second call replaces the first socket, which closes 4009. */
  connectDesktop(): FrameSocket {
    if (this.#desktop) this._closeEndpoint(this.#desktop, CloseCodes.REPLACED);
    const socket = new RelayEndpoint(this, "desktop", null);
    this.#desktop = socket;
    for (const device of this.#devices) this.#ctrl(device, { t: "presence", desktop: "online" });
    return socket;
  }

  /** A device end, authenticated as `deviceId` (the Worker did that before the upgrade). */
  connectDevice(deviceId: string): FrameSocket {
    const id = bytesToUuid(uuidToBytes(deviceId));
    const socket = new RelayEndpoint(this, "device", id);
    const sameDevice = [...this.#devices].filter((d) => d.deviceId === id).length;
    if (this.#devices.size >= this.#perRoom || sameDevice >= this.#perDevice) {
      this._defer(() => this._closeEndpoint(socket, CloseCodes.ROOM_QUOTA));
      return socket;
    }
    this.#devices.add(socket);
    this.#ctrl(socket, { t: "presence", desktop: this.#desktop ? "online" : "offline" });
    if (this.#desktop) this.#ctrl(this.#desktop, { t: "device", id, event: "joined" });
    return socket;
  }

  /** Server-side revocation: closes every socket of `deviceId` with `code` (default 4003). */
  revokeDevice(deviceId: string, code: number = CloseCodes.REVOKED): void {
    for (const device of [...this.#devices]) {
      if (device.deviceId === deviceId) this._closeEndpoint(device, code);
    }
  }

  /** Server-side close of the desktop socket. */
  closeDesktop(code: number = CloseCodes.GOING_AWAY): void {
    if (this.#desktop) this._closeEndpoint(this.#desktop, code);
  }

  /** Sends the `grants` CTRL frame to the desktop. */
  notifyGrants(): void {
    if (this.#desktop) this.#ctrl(this.#desktop, { t: "grants" });
  }

  /** Resolves when no delivery is queued, so a test can assert after a round trip. */
  async settled(): Promise<void> {
    for (let i = 0; i < 10_000 && this.#pending > 0; i++) await Promise.resolve();
    if (this.#pending > 0) throw new Error("the relay did not settle (a delivery loop?)");
  }

  // --- internals used by RelayEndpoint ---

  _defer(task: () => void): void {
    this.#pending += 1;
    queueMicrotask(() => {
      try {
        task();
      } finally {
        this.#pending -= 1;
      }
    });
  }

  _closeEndpoint(socket: RelayEndpoint, code: number): void {
    if (socket.closed) return;
    socket.closed = true;
    if (socket === this.#desktop) {
      this.#desktop = null;
      if (code !== CloseCodes.REPLACED) for (const device of this.#devices) this.#ctrl(device, { t: "presence", desktop: "offline" });
    } else if (this.#devices.delete(socket) && this.#desktop && socket.deviceId) {
      this.#ctrl(this.#desktop, { t: "device", id: socket.deviceId, event: "left" });
    }
    this._defer(() => socket.onclose?.(code));
  }

  _textFromSocket(from: RelayEndpoint, text: string): void {
    if (from.closed) return;
    if (text === ROOM_QUOTAS.textFramePing) from._deliverText(ROOM_QUOTAS.textFramePong);
    else this._closeEndpoint(from, CloseCodes.PROTOCOL_ERROR);
  }

  _fromSocket(from: RelayEndpoint, bytes: Uint8Array): void {
    if (from.closed) return;
    if (bytes.length > MAX_FRAME_SIZE) return this._closeEndpoint(from, CloseCodes.MESSAGE_TOO_BIG);
    if (bytes.length < MIN_FRAME_SIZE || bytes[0] !== FRAME_VERSION) return this._closeEndpoint(from, CloseCodes.PROTOCOL_ERROR);
    const type = bytes[1] as number;
    if (from.role === "device") {
      if (!deviceMaySend(type)) {
        this.dropped += 1;
        return;
      }
      const authenticated = overwriteDeviceId(bytes, from.deviceId as string);
      const frame = decodeFrame(authenticated);
      if (!this.#desktop) {
        const reply = encodeFrame({
          type: FrameType.ERROR,
          channel: frame.channel,
          deviceId: frame.deviceId,
          body: encodeErrorBody({ code: "desktop_offline" }),
        });
        from._deliver(reply);
        return;
      }
      this.forwarded.push({ direction: "device->desktop", frame });
      this.#desktop._deliver(authenticated);
      return;
    }
    // Desktop -> device(s), routed by the deviceId in the header.
    if (!desktopMaySend(type)) {
      this.dropped += 1;
      return;
    }
    const frame = decodeFrame(bytes);
    this.forwarded.push({ direction: "desktop->device", frame });
    for (const device of this.#devices) if (device.deviceId === frame.deviceId) device._deliver(bytes);
  }

  #ctrl(to: RelayEndpoint, message: CtrlMessage): void {
    // CTRL frames carry no meaningful channel or device id towards the desktop, so use zeros.
    const deviceId = to.deviceId ?? "00000000-0000-0000-0000-000000000000";
    to._deliver(encodeFrame({ type: FrameType.CTRL, channel: 0, deviceId, body: encodeCtrlBody(message) }));
  }
}

// --- WebSocket adapter ---

/** The part of the WebSocket API the adapter uses, so tests can inject a fake. */
export interface WebSocketLike {
  binaryType: string;
  send(data: Uint8Array | string): void;
  close(code?: number): void;
  onopen: (() => void) | null;
  onerror: ((event: unknown) => void) | null;
  onclose: ((event: { code: number }) => void) | null;
  onmessage: ((event: { data: unknown }) => void) | null;
}

export type WebSocketConstructor = new (url: string, init?: unknown) => WebSocketLike;

export interface ConnectRelayOptions {
  /** Default: the global `WebSocket`. */
  WebSocket?: WebSocketConstructor;
}

class WebSocketFrameSocket implements FrameSocket {
  onmessage: ((bytes: Uint8Array) => void) | null = null;
  ontext: ((text: string) => void) | null = null;
  onclose: ((code: number) => void) | null = null;
  closed = false;
  readonly #ws: WebSocketLike;

  constructor(ws: WebSocketLike) {
    this.#ws = ws;
    ws.onmessage = (event) => {
      const data = event.data;
      if (typeof data === "string") this.ontext?.(data);
      else if (data instanceof ArrayBuffer) this.onmessage?.(new Uint8Array(data));
      else if (ArrayBuffer.isView(data)) this.onmessage?.(new Uint8Array(data.buffer, data.byteOffset, data.byteLength));
    };
    ws.onclose = (event) => {
      this.closed = true;
      this.onclose?.(event.code);
    };
  }

  send(bytes: Uint8Array): void {
    this.#ws.send(bytes);
  }

  sendText(text: string): void {
    this.#ws.send(text);
  }

  close(code = CloseCodes.NORMAL): void {
    this.#ws.close(code);
  }
}

/**
 * Opens a relay WebSocket and resolves once it is open. `headers` (for example the
 * desktop `Authorization: Bearer ...` or a device `Cookie`) are passed in the init
 * object that Node's WebSocket accepts; a browser ignores it, which is fine because a
 * browser authenticates with its cookie. Rejects if the socket closes before opening.
 */
export function connectRelay(url: string, headers: Record<string, string> = {}, options: ConnectRelayOptions = {}): Promise<FrameSocket> {
  const Ctor = options.WebSocket ?? (globalThis as unknown as { WebSocket?: WebSocketConstructor }).WebSocket;
  if (!Ctor) return Promise.reject(new Error("no global WebSocket in this runtime"));
  return new Promise((resolve, reject) => {
    const ws = new Ctor(url, { headers });
    ws.binaryType = "arraybuffer";
    ws.onopen = () => resolve(new WebSocketFrameSocket(ws));
    ws.onerror = () => reject(new Error(`relay connection to ${url} failed`));
    ws.onclose = (event) => reject(new Error(`relay closed with code ${event.code} before opening`));
  });
}
