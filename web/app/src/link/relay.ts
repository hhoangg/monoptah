// The browser side of the relay socket (architecture.md 3.3 step 2, 4.4, 5.1).
//
// `RelayConnection` wraps any `RelaySocket`: a real WebSocket through `openRelay`, or the
// in-memory relay of @monoptah/mock-desktop in tests (its `FrameSocket` has the same
// shape). It validates incoming binary frames, sends the text keepalive `ping` and
// swallows `pong`. It knows nothing about Noise or channels. One connection serves one
// consumer: `onFrame` is a single callback.
import { PING_INTERVAL_MS, ROOM_QUOTAS, decodeFrame, encodeFrame, FrameType, type Frame } from "@monoptah/protocol";

/** One end of a relay connection. Compatible with the `FrameSocket` of @monoptah/mock-desktop. */
export interface RelaySocket {
  send(bytes: Uint8Array): void;
  sendText(text: string): void;
  /** Closes with a WebSocket close code. */
  close(code?: number): void;
  onmessage: ((bytes: Uint8Array) => void) | null;
  ontext: ((text: string) => void) | null;
  /** `reason` is absent on sockets that do not report it. */
  onclose: ((code: number, reason?: string) => void) | null;
  readonly closed: boolean;
}

/** Frames kept for a consumer that has not attached yet. */
const MAX_BACKLOG = 16;

export interface RelayConnectionOptions {
  /** Text `ping` period (default `PING_INTERVAL_MS`, 30 s). */
  pingIntervalMs?: number;
}

export class RelayConnection {
  /**
   * Called with every valid binary frame. Frames that fail `decodeFrame` are dropped. Frames that arrive
   * before the first consumer attaches (the join-time `presence` CTRL frame, 4.4) are kept, up to
   * `MAX_BACKLOG`, and delivered in order right after it does; once a consumer has attached and left
   * (`onFrame = null`), frames are dropped.
   */
  get onFrame(): ((frame: Frame) => void) | null {
    return this.#handler;
  }
  set onFrame(handler: ((frame: Frame) => void) | null) {
    this.#handler = handler;
    if (handler && !this.#attached) {
      this.#attached = true;
      if (this.#backlog.length > 0) {
        this.#flushing = true;
        queueMicrotask(() => this.#flush());
      }
    }
  }
  /** Called once when the socket closes (by either side). */
  onClose: ((code: number, reason: string) => void) | null = null;
  /** Count of binary messages dropped because `decodeFrame` rejected them. */
  invalidFrames = 0;

  readonly #socket: RelaySocket;
  #handler: ((frame: Frame) => void) | null = null;
  #attached = false;
  #flushing = false;
  readonly #backlog: Frame[] = [];
  #closed = false;
  #timer: ReturnType<typeof setInterval> | null = null;

  constructor(socket: RelaySocket, options: RelayConnectionOptions = {}) {
    this.#socket = socket;
    socket.onmessage = (bytes) => this.#onBinary(bytes);
    // `pong` is the only text the relay sends; it carries no information, and any other text is ignored.
    socket.ontext = () => undefined;
    socket.onclose = (code, reason) => this.#onClose(code, reason ?? "");
    if (socket.closed) {
      // Already closed before we attached: report it asynchronously like a real socket.
      queueMicrotask(() => this.#onClose(1006, ""));
      return;
    }
    this.#timer = setInterval(() => this.#ping(), options.pingIntervalMs ?? PING_INTERVAL_MS);
  }

  get closed(): boolean {
    return this.#closed;
  }

  /** Encodes and sends one frame. Ignored after close. */
  send(frame: { type: Frame["type"]; channel: number; deviceId: string; body?: Uint8Array }): void {
    if (this.#closed) return;
    this.#socket.send(encodeFrame(frame));
  }

  /** Closes the socket and stops the keepalive. `onClose` is called once when the socket reports the close. */
  close(code = 1000): void {
    this.#stopTimer();
    if (this.#closed) return;
    this.#socket.close(code);
  }

  /**
   * Drops the buffered frames that cannot belong to a session that has not sent its HS1 yet (`BYE`, `HS2`,
   * `TRANSPORT`: they answer or end an earlier session on the same channel). `CTRL` and `ERROR` stay.
   */
  discardStaleBacklog(): void {
    const keep = this.#backlog.filter((frame) => frame.type === FrameType.CTRL || frame.type === FrameType.ERROR);
    this.#backlog.splice(0, this.#backlog.length, ...keep);
  }

  #ping(): void {
    if (this.#closed) return;
    this.#socket.sendText(ROOM_QUOTAS.textFramePing);
  }

  #stopTimer(): void {
    if (this.#timer !== null) clearInterval(this.#timer);
    this.#timer = null;
  }

  #onBinary(bytes: Uint8Array): void {
    let frame: Frame;
    try {
      frame = decodeFrame(bytes);
    } catch {
      this.invalidFrames += 1;
      return;
    }
    if (!this.#attached || this.#flushing) {
      if (this.#backlog.length < MAX_BACKLOG) this.#backlog.push(frame);
      return;
    }
    this.#handler?.(frame);
  }

  #flush(): void {
    const frames = this.#backlog.splice(0);
    this.#flushing = false;
    for (const frame of frames) this.#handler?.(frame);
  }

  #onClose(code: number, reason: string): void {
    if (this.#closed) return;
    this.#closed = true;
    this.#stopTimer();
    this.onClose?.(code, reason);
  }
}

// --- WebSocket adapter ---

/** The part of the WebSocket API this module uses, so tests can inject a fake. */
export interface WebSocketLike {
  binaryType: string;
  readyState?: number;
  send(data: Uint8Array | string): void;
  close(code?: number): void;
  onopen: (() => void) | null;
  onerror: ((event: unknown) => void) | null;
  onclose: ((event: { code: number; reason?: string }) => void) | null;
  onmessage: ((event: { data: unknown }) => void) | null;
}
export type WebSocketConstructor = new (url: string) => WebSocketLike;

/** The part of `location` the URL needs. */
export interface LocationLike {
  protocol: string;
  host: string;
  hostname: string;
}

/**
 * `wss://HOST/relay/device/:desktopId?device=<deviceId>`; `ws://` when the page is served from
 * `http://localhost` (dev). Built from the page location.
 */
export function relayUrl(desktopId: string, deviceId: string, location: LocationLike = globalThis.location): string {
  const secure = !(location.protocol === "http:" && (location.hostname === "localhost" || location.hostname === "127.0.0.1"));
  const scheme = secure ? "wss" : "ws";
  return `${scheme}://${location.host}/relay/device/${encodeURIComponent(desktopId)}?device=${encodeURIComponent(deviceId)}`;
}

/** The WebSocket never opened. `closeCode` is set when the server closed it (a browser reports 1006 when the upgrade is refused). */
export class RelayConnectError extends Error {
  readonly closeCode: number | undefined;
  constructor(message: string, closeCode?: number) {
    super(message);
    this.name = "RelayConnectError";
    this.closeCode = closeCode;
  }
}

class WebSocketRelaySocket implements RelaySocket {
  onmessage: ((bytes: Uint8Array) => void) | null = null;
  ontext: ((text: string) => void) | null = null;
  onclose: ((code: number, reason?: string) => void) | null = null;
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
    ws.onerror = () => undefined;
    ws.onclose = (event) => {
      this.closed = true;
      this.onclose?.(event.code, event.reason ?? "");
    };
  }

  send(bytes: Uint8Array): void {
    this.#ws.send(bytes);
  }

  sendText(text: string): void {
    this.#ws.send(text);
  }

  close(code = 1000): void {
    this.#ws.close(code);
  }
}

export interface OpenRelayOptions {
  desktopId: string;
  deviceId: string;
  /** Default: the global `WebSocket`. */
  WebSocket?: WebSocketConstructor;
  /** Default: the global `location`. */
  location?: LocationLike;
  pingIntervalMs?: number;
  /** The upgrade must complete within this time (default `OPEN_TIMEOUT_MS`); then the socket is closed and the open rejects. */
  openTimeoutMs?: number;
}

/** Same as the Noise handshake timeout of `LinkSession` (30 s). */
export const OPEN_TIMEOUT_MS = 30_000;

/** Opens the relay WebSocket and resolves once it is open. Rejects with `RelayConnectError` if it closes or fails first. */
export function openRelay(options: OpenRelayOptions): Promise<RelayConnection> {
  const Ctor = options.WebSocket ?? (globalThis as { WebSocket?: WebSocketConstructor }).WebSocket;
  if (!Ctor) return Promise.reject(new RelayConnectError("no WebSocket in this runtime"));
  const url = relayUrl(options.desktopId, options.deviceId, options.location);
  return new Promise((resolve, reject) => {
    let ws: WebSocketLike;
    try {
      ws = new Ctor(url);
    } catch {
      reject(new RelayConnectError("could not open the relay socket"));
      return;
    }
    ws.binaryType = "arraybuffer";
    const timer = setTimeout(() => {
      ws.onopen = null;
      ws.onclose = null;
      try {
        ws.close(1000);
      } catch {
        // Already closed.
      }
      reject(new RelayConnectError("relay did not open in time"));
    }, options.openTimeoutMs ?? OPEN_TIMEOUT_MS);
    ws.onopen = () => {
      clearTimeout(timer);
      const socket = new WebSocketRelaySocket(ws);
      resolve(new RelayConnection(socket, options.pingIntervalMs === undefined ? {} : { pingIntervalMs: options.pingIntervalMs }));
    };
    ws.onerror = () => undefined;
    ws.onclose = (event) => {
      clearTimeout(timer);
      reject(new RelayConnectError(`relay closed with code ${event.code} before opening`, event.code));
    };
  });
}
