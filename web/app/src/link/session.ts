// One Noise channel (desktopId, deviceId, channel) over a `RelayConnection`
// (architecture.md 3.3 steps 3 to 6, 5.3 to 5.5): the IK handshake as initiator, the
// `state` message, requests and watches with `rid`, and the end of the session.
// A session never resumes: after `onEnd` a new session is a new handshake.
// Pure TypeScript: no React, no Zustand. Keys never leave this file's handshake objects
// and are never logged or put in errors.
import {
  CtrlMessageSchema,
  DesktopMessageSchema,
  ENVELOPE_VERSION,
  ErrorBodySchema,
  FrameType,
  MAX_COMMAND_ID_LENGTH,
  MAX_REQUEST_BYTES,
  PROTOCOL_VERSION,
  REQUEST_TIMEOUT_MS,
  DEAD_AFTER_MS,
  WatchFrameSchema,
  decodeEnvelopeJson,
  encodeEnvelope,
  type DeviceMessage,
  type EnvelopeRequest,
  type EnvelopeResponse,
  type Frame,
  type MethodName,
  type ParamsOf,
  type PresenceTarget,
  type ResultOf,
  type StateMessage,
  type WatchFrame,
} from "@monoptah/protocol";
import { bytesToHex, randomBytes } from "@noble/hashes/utils.js";
import { HandshakeState, prologue, type KeyPair } from "./noise.ts";
import { SecureChannel } from "./secure-channel.ts";
import type { RelayConnection } from "./relay.ts";

/** Codes of `LinkRequestError` raised by the client itself; a desktop `code` (for example `read_only`) is passed through as it is. */
export const LINK_ERROR_CODES = [
  "timeout",
  "not_ready",
  "session_ended",
  "too_large",
  "invalid_id",
  "aborted",
  "invalid_frame",
] as const;
export type LinkErrorCode = (typeof LINK_ERROR_CODES)[number];

/** A request or watch failed. `code` is a desktop code (`read_only`, `busy`, ...) or one of `LINK_ERROR_CODES`. */
export class LinkRequestError extends Error {
  readonly code: string | undefined;
  constructor(message: string, code?: string) {
    super(message);
    this.name = "LinkRequestError";
    this.code = code;
  }
}

/** Why a session ended (`onEnd`) or never became ready (`LinkSessionError.reason`). */
export type LinkEndReason =
  /** BYE from the desktop (revoked device, wrong key, unknown channel, protocol breach on its side). */
  | "bye"
  /** A TRANSPORT body did not decrypt or reassemble. */
  | "decrypt_failed"
  /** The desktop sent something that is not valid protocol. */
  | "protocol_error"
  /** The desktop `proto` is higher than this build knows (architecture.md 5.6): show "Reload to update the web app". */
  | "unsupported_proto"
  /** The desktop `proto` (HS2 payload) is lower than this build speaks: show "Update Monoptah on your desktop". */
  | "desktop_proto_older"
  /** The relay socket closed; `detail.closeCode` and `closeReason` say how. */
  | "relay_closed"
  /** `close()` was called. */
  | "closed"
  /** Only before the first `state`: no `state` arrived in time. */
  | "handshake_timeout"
  /** Only before the first `state`: the relay answered the handshake with an ERROR frame (`detail.relayCode`). */
  | "relay_error";

export interface LinkEndDetail {
  closeCode?: number;
  closeReason?: string;
  relayCode?: string;
}

/** `LinkSession.connect` failed before the first `state`. */
export class LinkSessionError extends Error {
  readonly reason: LinkEndReason;
  readonly detail: LinkEndDetail;
  constructor(reason: LinkEndReason, detail: LinkEndDetail = {}) {
    super(`link session did not start: ${reason}`);
    this.name = "LinkSessionError";
    this.reason = reason;
    this.detail = detail;
  }
}

export interface LinkSessionOptions {
  relay: RelayConnection;
  /** UUID strings. */
  desktopId: string;
  deviceId: string;
  /** u16 chosen per tab (default 1). */
  channel?: number;
  /** The pinned desktop static public key. */
  desktopPublicKey: Uint8Array;
  /**
   * The device static key pair, only used for the handshake. The session TAKES OWNERSHIP of
   * `deviceKey.privateKey`: it zeroes that buffer when the handshake is over (or the session ends first),
   * so pass a copy and never the module buffer of keys.ts.
   */
  deviceKey: KeyPair;
  /** Sent encrypted in HS1 (shown on the desktop as untrusted text). */
  deviceName: string;
  /** Short user agent for HS1 (default: `navigator.userAgent`, cut to 160 characters). */
  userAgent?: string;
  /** Called for every frame on this session's channel (not CTRL), for the liveness display of the app. */
  onActivity?: () => void;
  /** Called for every `state` message, the first one included. */
  onState?: (state: StateMessage) => void;
  /** Called once when the session ends after it became ready. */
  onEnd?: (reason: LinkEndReason, detail: LinkEndDetail) => void;
  /** A relay ERROR frame for this channel (`desktop_offline`, `unsupported_version`, or a code this build does not know). */
  onRelayError?: (code: string) => void;
  /** Relay CTRL `presence`. */
  onPresence?: (desktop: "online" | "offline") => void;
  /** Close the relay socket when the session ends (default true: one socket per session). */
  closeRelayOnEnd?: boolean;
  /** No `state` within this time of connecting rejects `connect` (default 30 s). */
  readyTimeoutMs?: number;
  requestTimeoutMs?: number;
  deadAfterMs?: number;
  rekeyAfterMessages?: number;
  rekeyAfterMs?: number;
  /** Clock for `lastFrameAt` and the rekey schedule (default `Date.now`). */
  now?: () => number;
}

export interface RequestOptions {
  /** Command id (1 to 128 chars). Default: a fresh random id. */
  id?: string;
  signal?: AbortSignal;
}

export interface WatchOptions extends RequestOptions {
  /** Silence of `deadAfterMs` (75 s) with no frame and no heartbeat: the watch is closed (a `cancel` is sent) and this is called. */
  onDead?: () => void;
  /** The desktop ended the watch with an error response, or sent a frame this build cannot read. */
  onError?: (error: LinkRequestError) => void;
}

export interface WatchHandle {
  readonly rid: number;
  /** True once stopped, dead or ended by an error or by the session. */
  readonly closed: boolean;
  /** Sends `{rid, cancel: true}` and ends the watch. */
  stop(): void;
}

interface PendingRequest {
  resolve(result: unknown): void;
  reject(error: LinkRequestError): void;
  timer: ReturnType<typeof setTimeout>;
  cleanup: () => void;
}

interface WatchEntry {
  rid: number;
  onFrame: (frame: WatchFrame) => void;
  options: WatchOptions;
  /** Settles `watch()` on the `watching` ack. */
  ack: { resolve(handle: WatchHandle): void; reject(error: LinkRequestError): void; timer: ReturnType<typeof setTimeout> } | null;
  deadTimer: ReturnType<typeof setTimeout> | null;
  handle: WatchHandle;
  closed: boolean;
  cleanup: () => void;
}

const MAX_USER_AGENT = 160;

function randomId(): string {
  return bytesToHex(randomBytes(16));
}

function shortUserAgent(): string {
  const ua = (globalThis as { navigator?: { userAgent?: string } }).navigator?.userAgent;
  return (ua ?? "unknown").slice(0, MAX_USER_AGENT);
}

export class LinkSession {
  readonly desktopId: string;
  readonly deviceId: string;
  readonly channel: number;
  /** The latest `state` message (always set once `connect` resolved). */
  state: StateMessage;
  /** `now()` at the last frame on this session's channel (CTRL frames and other tabs' channels do not count); the app uses it for liveness. */
  lastFrameAt: number;
  onState: ((state: StateMessage) => void) | null;
  onEnd: ((reason: LinkEndReason, detail: LinkEndDetail) => void) | null;
  onRelayError: ((code: string) => void) | null;
  onPresence: ((desktop: "online" | "offline") => void) | null;

  readonly #options: Omit<LinkSessionOptions, "deviceKey">;
  #deviceKey: KeyPair | null;
  readonly #relay: RelayConnection;
  readonly #now: () => number;
  readonly #requestTimeoutMs: number;
  readonly #deadAfterMs: number;
  readonly #pending = new Map<number, PendingRequest>();
  readonly #watches = new Map<number, WatchEntry>();
  #handshake: HandshakeState | null = null;
  #crypto: SecureChannel | null = null;
  #nextRid = 1;
  #ended = false;
  #ready = false;
  #readyTimer: ReturnType<typeof setTimeout> | null = null;
  #readyResolve: ((session: LinkSession) => void) | null = null;
  #readyReject: ((error: LinkSessionError) => void) | null = null;
  #reassemblyTimer: ReturnType<typeof setInterval> | null = null;

  private constructor(options: LinkSessionOptions, initialState: StateMessage) {
    const { deviceKey, ...rest } = options;
    this.#options = rest;
    this.#deviceKey = deviceKey;
    this.#relay = options.relay;
    this.desktopId = options.desktopId;
    this.deviceId = options.deviceId;
    this.channel = options.channel ?? 1;
    this.state = initialState;
    this.#now = options.now ?? Date.now;
    this.lastFrameAt = this.#now();
    this.#requestTimeoutMs = options.requestTimeoutMs ?? REQUEST_TIMEOUT_MS;
    this.#deadAfterMs = options.deadAfterMs ?? DEAD_AFTER_MS;
    this.onState = options.onState ?? null;
    this.onEnd = options.onEnd ?? null;
    this.onRelayError = options.onRelayError ?? null;
    this.onPresence = options.onPresence ?? null;
  }

  /**
   * Handshakes as initiator and waits for the first `state` message. Resolves with the session
   * whatever the state (`pending` and `revoked` are states too; a revoked device normally gets
   * BYE, so `connect` rejects with `LinkSessionError("bye")`). Rejects with `LinkSessionError`
   * on BYE, a relay ERROR, a failed handshake, an unsupported `proto`, a closed socket or a timeout.
   */
  static connect(options: LinkSessionOptions): Promise<LinkSession> {
    const session = new LinkSession(options, { type: "state", state: "pending" });
    return session.#start();
  }

  /** Zeroes the device private key this session owns. */
  #wipeDeviceKey(): void {
    this.#deviceKey?.privateKey.fill(0);
    this.#deviceKey = null;
  }

  #start(): Promise<LinkSession> {
    const options = this.#options;
    try {
      this.#handshake = new HandshakeState({
        initiator: true,
        prologue: prologue(this.desktopId, this.deviceId, this.channel),
        s: this.#deviceKey as KeyPair,
        rs: options.desktopPublicKey,
      });
    } catch (error) {
      this.#wipeDeviceKey();
      throw error;
    }
    const ready = new Promise<LinkSession>((resolve, reject) => {
      this.#readyResolve = resolve;
      this.#readyReject = reject;
    });
    // Frames buffered before this session's HS1 (a late BYE of an earlier session on this channel) are not ours.
    this.#relay.discardStaleBacklog();
    this.#relay.onFrame = (frame) => this.#onFrame(frame);
    this.#relay.onClose = (code, reason) => this.#finish("relay_closed", { closeCode: code, closeReason: reason }, false);
    this.#readyTimer = setTimeout(() => this.#finish("handshake_timeout", {}, true), options.readyTimeoutMs ?? 30_000);
    try {
      const payload = new TextEncoder().encode(JSON.stringify({ name: options.deviceName, ua: options.userAgent ?? shortUserAgent() }));
      const hs1 = this.#handshake.writeMessage(payload);
      this.#relay.send({ type: FrameType.HS1, channel: this.channel, deviceId: this.deviceId, body: hs1 });
    } catch {
      this.#finish("protocol_error", {}, true);
    }
    return ready;
  }

  /** True while the session can send requests (state `confirmed`, not ended). */
  get ready(): boolean {
    return !this.#ended && this.state.state === "confirmed";
  }

  get ended(): boolean {
    return this.#ended;
  }

  /** Rekeys this side has sent and received (for tests and diagnostics). */
  get rekeysSent(): number {
    return this.#crypto?.rekeysSent ?? 0;
  }
  get rekeysReceived(): number {
    return this.#crypto?.rekeysReceived ?? 0;
  }

  // --- Requests ---

  /** Sends a request and resolves with `result`. Rejects with `LinkRequestError`. */
  request<M extends MethodName>(method: M, params?: ParamsOf<M>, options?: RequestOptions): Promise<ResultOf<M>>;
  request(method: string, params?: Record<string, unknown>, options?: RequestOptions): Promise<unknown>;
  request(method: string, params: unknown = {}, options: RequestOptions = {}): Promise<unknown> {
    return new Promise((resolve, reject) => {
      const prepared = this.#prepare(method, params as Record<string, unknown>, options, false);
      if (prepared instanceof LinkRequestError) return reject(prepared);
      const { rid, bytes } = prepared;
      const signal = options.signal;
      const onAbort = (): void => {
        if (this.#pending.delete(rid)) {
          cleanup();
          reject(new LinkRequestError("The request was aborted", "aborted"));
        }
      };
      const timer = setTimeout(() => {
        if (this.#pending.delete(rid)) {
          cleanup();
          reject(new LinkRequestError("The desktop did not answer in time", "timeout"));
        }
      }, this.#requestTimeoutMs);
      const cleanup = (): void => {
        clearTimeout(timer);
        signal?.removeEventListener("abort", onAbort);
      };
      this.#pending.set(rid, { resolve, reject, timer, cleanup });
      signal?.addEventListener("abort", onAbort, { once: true });
      if (!this.#sendBytes(bytes)) {
        this.#pending.delete(rid);
        cleanup();
        reject(new LinkRequestError("The session has ended", "session_ended"));
      }
    });
  }

  /**
   * Opens a watch (`conversation.watch`). Resolves after the `watching` ack with a handle; frames go to
   * `onFrame`. Rejects with `LinkRequestError` when the desktop refuses or does not ack in time.
   */
  watch(method: string, params: Record<string, unknown>, onFrame: (frame: WatchFrame) => void, options: WatchOptions = {}): Promise<WatchHandle> {
    return new Promise((resolve, reject) => {
      const prepared = this.#prepare(method, params, options, true);
      if (prepared instanceof LinkRequestError) return reject(prepared);
      const { rid, bytes } = prepared;
      const signal = options.signal;
      const entry: WatchEntry = {
        rid,
        onFrame,
        options,
        ack: null,
        deadTimer: null,
        closed: false,
        cleanup: () => undefined,
        handle: {
          rid,
          get closed() {
            return entry.closed;
          },
          stop: () => this.#stopWatch(entry, true),
        },
      };
      const onAbort = (): void => {
        if (entry.ack) {
          const { reject: rej } = entry.ack;
          this.#stopWatch(entry, true);
          rej(new LinkRequestError("The request was aborted", "aborted"));
        } else this.#stopWatch(entry, true);
      };
      entry.cleanup = () => signal?.removeEventListener("abort", onAbort);
      entry.ack = {
        resolve,
        reject,
        timer: setTimeout(() => {
          if (!entry.ack) return;
          const { reject: rej } = entry.ack;
          this.#stopWatch(entry, true);
          rej(new LinkRequestError("The desktop did not answer in time", "timeout"));
        }, this.#requestTimeoutMs),
      };
      this.#watches.set(rid, entry);
      signal?.addEventListener("abort", onAbort, { once: true });
      if (!this.#sendBytes(bytes)) {
        this.#stopWatch(entry, false);
        reject(new LinkRequestError("The session has ended", "session_ended"));
      }
    });
  }

  /**
   * Sends a presence report (what this page shows, or null). Fire and forget: it has no `rid`, gets no answer and
   * an older desktop drops it. Not sent before the device is confirmed.
   */
  sendPresence(target: PresenceTarget | null): boolean {
    if (this.#ended || this.state.state !== "confirmed" || !this.#crypto) return false;
    return this.#sendBytes(encodeEnvelope({ t: "presence", version: ENVELOPE_VERSION, target }));
  }

  /** Ends the session: BYE to the desktop, pending requests reject with `session_ended`, `onEnd("closed")`. */
  close(): void {
    this.#finish("closed", {}, true);
  }

  // --- Request plumbing ---

  /** Validates and allocates; returns the encoded envelope or the error to reject with. */
  #prepare(method: string, params: Record<string, unknown>, options: RequestOptions, watch: boolean): { rid: number; bytes: Uint8Array } | LinkRequestError {
    if (this.#ended) return new LinkRequestError("The session has ended", "session_ended");
    if (this.state.state !== "confirmed" || !this.#crypto) return new LinkRequestError("This device is not confirmed on the desktop yet", "not_ready");
    const id = options.id ?? randomId();
    if (id.length < 1 || id.length > MAX_COMMAND_ID_LENGTH) return new LinkRequestError(`The command id must be 1 to ${MAX_COMMAND_ID_LENGTH} characters`, "invalid_id");
    const rid = this.#allocateRid();
    const request: EnvelopeRequest = { version: ENVELOPE_VERSION, rid, id, method, params, ...(watch ? { watch: true as const } : {}) };
    const bytes = encodeEnvelope(request);
    if (bytes.length > MAX_REQUEST_BYTES) return new LinkRequestError("The request is too large", "too_large");
    return { rid, bytes };
  }

  #allocateRid(): number {
    for (;;) {
      const rid = this.#nextRid;
      this.#nextRid = (this.#nextRid + 1) >>> 0;
      if (!this.#pending.has(rid) && !this.#watches.has(rid)) return rid;
    }
  }

  /** Seals and sends one message. Returns false when the session cannot send. */
  #sendBytes(bytes: Uint8Array): boolean {
    const crypto = this.#crypto;
    if (this.#ended || !crypto) return false;
    let bodies: Uint8Array[];
    try {
      bodies = crypto.seal(bytes);
    } catch {
      this.#finish("protocol_error", {}, true);
      return false;
    }
    for (const body of bodies) this.#relay.send({ type: FrameType.TRANSPORT, channel: this.channel, deviceId: this.deviceId, body });
    return true;
  }

  #sendMessage(message: DeviceMessage): void {
    this.#sendBytes(encodeEnvelope(message));
  }

  // --- Incoming frames ---

  #onFrame(frame: Frame): void {
    if (this.#ended) return;
    if (frame.type === FrameType.CTRL) return this.#onCtrl(frame);
    if (frame.channel !== this.channel) return; // another tab's channel on a shared socket
    this.lastFrameAt = this.#now();
    this.#options.onActivity?.();
    switch (frame.type) {
      case FrameType.ERROR:
        return this.#onRelayError(frame);
      case FrameType.BYE:
        return this.#finish("bye", {}, false);
      case FrameType.HS2:
        return this.#onHandshake(frame);
      case FrameType.TRANSPORT:
        return this.#onTransport(frame);
      default:
        return;
    }
  }

  #onCtrl(frame: Frame): void {
    let message;
    try {
      message = CtrlMessageSchema.parse(JSON.parse(new TextDecoder().decode(frame.body)));
    } catch {
      return;
    }
    if (message.t === "presence") this.onPresence?.(message.desktop);
  }

  #onRelayError(frame: Frame): void {
    let code: string;
    try {
      code = ErrorBodySchema.parse(JSON.parse(new TextDecoder().decode(frame.body))).code;
    } catch {
      return;
    }
    if (!this.#ready) {
      this.#finish("relay_error", { relayCode: code }, true);
      this.onRelayError?.(code);
      return;
    }
    this.onRelayError?.(code);
  }

  #onHandshake(frame: Frame): void {
    const handshake = this.#handshake;
    if (!handshake || this.#crypto) return;
    try {
      const payload = handshake.readMessage(frame.body);
      const proto = (JSON.parse(new TextDecoder().decode(payload)) as { proto?: unknown } | null)?.proto;
      if (typeof proto !== "number" || !Number.isInteger(proto) || proto < 1) throw new Error("bad proto");
      if (proto > PROTOCOL_VERSION) return this.#finish("unsupported_proto", {}, true);
      if (proto < PROTOCOL_VERSION) return this.#finish("desktop_proto_older", {}, true);
      const { send, recv } = handshake.split();
      this.#handshake = null;
      this.#wipeDeviceKey();
      // The crypto must exist before the next frame (the state message) is delivered.
      this.#crypto = new SecureChannel({
        send,
        recv,
        maxOutgoingBytes: MAX_REQUEST_BYTES,
        now: this.#now,
        ...(this.#options.rekeyAfterMessages === undefined ? {} : { rekeyAfterMessages: this.#options.rekeyAfterMessages }),
        ...(this.#options.rekeyAfterMs === undefined ? {} : { rekeyAfterMs: this.#options.rekeyAfterMs }),
      });
      this.#reassemblyTimer = setInterval(() => this.#checkReassembly(), 5_000);
    } catch {
      this.#finish("protocol_error", {}, true);
    }
  }

  #checkReassembly(): void {
    try {
      this.#crypto?.checkTimeouts();
    } catch {
      this.#finish("protocol_error", {}, true);
    }
  }

  #onTransport(frame: Frame): void {
    const crypto = this.#crypto;
    if (!crypto) return;
    let message;
    try {
      const result = crypto.open(frame.body);
      if (result.type !== "message") return;
      message = DesktopMessageSchema.parse(decodeEnvelopeJson(result.bytes));
    } catch (error) {
      // Decrypt, reassembly, JSON and schema failures all end the session (architecture.md 5.4).
      const name = error instanceof Error ? error.name : "";
      return this.#finish(name === "NoiseError" ? "decrypt_failed" : "protocol_error", {}, true);
    }
    if ("type" in message && message.type === "state") return this.#onStateMessage(message);
    this.#onResponse(message as EnvelopeResponse);
  }

  #onStateMessage(message: StateMessage): void {
    if (message.proto !== undefined && message.proto > PROTOCOL_VERSION) return this.#finish("unsupported_proto", {}, true);
    this.state = message;
    if (!this.#ready) {
      this.#ready = true;
      if (this.#readyTimer !== null) clearTimeout(this.#readyTimer);
      this.#readyTimer = null;
      const resolve = this.#readyResolve;
      this.#readyResolve = null;
      this.#readyReject = null;
      this.onState?.(message);
      resolve?.(this);
      return;
    }
    this.onState?.(message);
  }

  #onResponse(response: EnvelopeResponse): void {
    const watch = this.#watches.get(response.rid);
    if (watch) return this.#onWatchResponse(watch, response);
    const pending = this.#pending.get(response.rid);
    if (!pending) return; // late answer to a timed-out or aborted request
    this.#pending.delete(response.rid);
    pending.cleanup();
    if (!response.ok) pending.reject(new LinkRequestError(response.error, response.code));
    else if ("result" in response) pending.resolve(response.result);
    else pending.reject(new LinkRequestError("Unexpected response", "invalid_frame"));
  }

  #onWatchResponse(watch: WatchEntry, response: EnvelopeResponse): void {
    if (!response.ok) {
      const error = new LinkRequestError(response.error, response.code);
      const ack = watch.ack;
      this.#stopWatch(watch, false);
      if (ack) ack.reject(error);
      else watch.options.onError?.(error);
      return;
    }
    if ("watching" in response) {
      const ack = watch.ack;
      if (!ack) return;
      clearTimeout(ack.timer);
      watch.ack = null;
      this.#armDead(watch);
      ack.resolve(watch.handle);
      return;
    }
    if (watch.ack) return; // frames or heartbeats before the ack are not expected
    this.#armDead(watch);
    if ("heartbeat" in response) return;
    const parsed = WatchFrameSchema.safeParse((response as { result: unknown }).result);
    if (!parsed.success) {
      this.#stopWatch(watch, true);
      watch.options.onError?.(new LinkRequestError("The desktop sent a watch frame this app cannot read", "invalid_frame"));
      return;
    }
    watch.onFrame(parsed.data);
  }

  #armDead(watch: WatchEntry): void {
    if (watch.deadTimer !== null) clearTimeout(watch.deadTimer);
    watch.deadTimer = setTimeout(() => {
      if (watch.closed) return;
      this.#stopWatch(watch, true);
      watch.options.onDead?.();
    }, this.#deadAfterMs);
  }

  /** Removes a watch; optionally tells the desktop. Does not settle `ack` (callers do). */
  #stopWatch(watch: WatchEntry, sendCancel: boolean): void {
    if (watch.closed) return;
    watch.closed = true;
    if (watch.deadTimer !== null) clearTimeout(watch.deadTimer);
    watch.deadTimer = null;
    if (watch.ack) clearTimeout(watch.ack.timer);
    watch.ack = null;
    watch.cleanup();
    this.#watches.delete(watch.rid);
    if (sendCancel && !this.#ended) this.#sendMessage({ rid: watch.rid, cancel: true });
  }

  // --- End of session ---

  /** Ends the session once. `sendBye` tells the desktop (never after BYE or a closed socket). */
  #finish(reason: LinkEndReason, detail: LinkEndDetail, sendBye: boolean): void {
    if (this.#ended) return;
    this.#ended = true;
    if (sendBye && !this.#relay.closed) this.#relay.send({ type: FrameType.BYE, channel: this.channel, deviceId: this.deviceId });
    if (this.#readyTimer !== null) clearTimeout(this.#readyTimer);
    this.#readyTimer = null;
    if (this.#reassemblyTimer !== null) clearInterval(this.#reassemblyTimer);
    this.#reassemblyTimer = null;
    this.#crypto?.reset();
    this.#handshake = null;
    this.#wipeDeviceKey();

    const ended = new LinkRequestError("The session has ended", "session_ended");
    for (const pending of this.#pending.values()) {
      pending.cleanup();
      pending.reject(ended);
    }
    this.#pending.clear();
    for (const watch of [...this.#watches.values()]) {
      const ack = watch.ack;
      this.#stopWatch(watch, false);
      if (ack) ack.reject(ended);
      else watch.options.onError?.(ended);
    }
    this.#relay.onFrame = null;
    if (this.#options.closeRelayOnEnd !== false && !this.#relay.closed) this.#relay.close();
    this.#relay.onClose = null;

    if (!this.#ready) {
      const reject = this.#readyReject;
      this.#readyResolve = null;
      this.#readyReject = null;
      reject?.(new LinkSessionError(reason, detail));
      return;
    }
    this.onEnd?.(reason, detail);
  }
}
