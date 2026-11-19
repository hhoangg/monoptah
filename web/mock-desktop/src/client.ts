// A minimal device-side client (Noise initiator) over a `FrameSocket`, for tests. It
// handshakes, receives `state`, sends requests with `rid`, reassembles chunks, opens
// watches and rekeys. Exported as `@monoptah/mock-desktop/test-client`.
// This is the "TS client" of the contract (g) freeze rule, not the browser link client
// of R-10 (that one adds reconnect, key storage and pinning on top).
import {
  CtrlMessageSchema,
  DesktopMessageSchema,
  ErrorBodySchema,
  FrameType,
  ENVELOPE_VERSION,
  decodeEnvelopeJson,
  decodeFrame,
  encodeEnvelope,
  encodeFrame,
  type EnvelopeRequest,
  type EnvelopeResponse,
  type Frame,
  type MethodName,
  type ParamsOf,
  type ResultOf,
  type StateMessage,
  type WatchFrame,
} from "@monoptah/protocol";
import { HandshakeState, generateKeyPair, prologue, type KeyPair } from "./noise.ts";
import { SecureChannel } from "./secure-channel.ts";
import type { FrameSocket } from "./transport.ts";

export interface TestClientOptions {
  socket: FrameSocket;
  /** UUID strings. */
  desktopId: string;
  deviceId: string;
  channel?: number;
  /** The pinned desktop static public key (from the directory). */
  desktopPublicKey: Uint8Array;
  /** The device static key; default: a fresh random pair. */
  deviceKey?: KeyPair;
  /** Sent encrypted in HS1 and shown on the desktop as untrusted text. */
  deviceName?: string;
  rekeyAfterMessages?: number;
  rekeyAfterMs?: number;
  now?: () => number;
  ephemeralPrivateKey?: Uint8Array;
}

/** Thrown by `connect` and `call` when the desktop or the relay refuses. */
export class ClientError extends Error {
  readonly code: string | undefined;
  constructor(message: string, code?: string) {
    super(message);
    this.name = "ClientError";
    this.code = code;
  }
}

export interface ClientWatch {
  rid: number;
  frames: WatchFrame[];
  heartbeats: number;
  onFrame: ((frame: WatchFrame) => void) | null;
  /** Sends `{rid, cancel: true}`. */
  cancel(): void;
}

interface Pending {
  resolve(response: EnvelopeResponse): void;
  reject(error: Error): void;
}

export class TestClient {
  readonly deviceKey: KeyPair;
  readonly deviceId: string;
  readonly channel: number;
  /** The latest `state` message, and every one received. */
  state: StateMessage | null = null;
  readonly states: StateMessage[] = [];
  readonly ctrl: unknown[] = [];
  /** The desktop (or relay) ended the session. */
  byeReceived = false;
  closeCode: number | null = null;
  /** Why the session broke, when a received frame failed to decrypt or parse. */
  failure: Error | null = null;
  onState: ((state: StateMessage) => void) | null = null;

  readonly #socket: FrameSocket;
  #crypto: SecureChannel | null = null;
  readonly #pending = new Map<number, Pending>();
  readonly #watches = new Map<number, ClientWatch>();
  #nextRid = 1;
  #handshake: HandshakeState | null = null;
  /** Settles `connect`: resolved by the first `state`, rejected by a BYE, a relay ERROR or a failed handshake. */
  #readyResolve: (() => void) | null = null;
  #readyReject: ((error: Error) => void) | null = null;

  readonly options: TestClientOptions;

  private constructor(options: TestClientOptions) {
    this.options = options;
    this.#socket = options.socket;
    this.deviceId = options.deviceId;
    this.channel = options.channel ?? 1;
    this.deviceKey = options.deviceKey ?? generateKeyPair();
  }

  /** Handshakes and waits for the first `state` message. Rejects on BYE, a relay ERROR or a failed handshake. */
  static async connect(options: TestClientOptions): Promise<TestClient> {
    const client = new TestClient(options);
    await client.#start();
    return client;
  }

  async #start(): Promise<void> {
    const handshake = new HandshakeState({
      initiator: true,
      prologue: prologue(this.options.desktopId, this.deviceId, this.channel),
      s: this.deviceKey,
      rs: this.options.desktopPublicKey,
      ...(this.options.ephemeralPrivateKey ? { ephemeralPrivateKey: this.options.ephemeralPrivateKey } : {}),
    });
    this.#handshake = handshake;
    this.#socket.onmessage = (bytes) => this.#onFrame(bytes);
    this.#socket.onclose = (code) => {
      this.closeCode = code;
      this.#failAll(new ClientError(`socket closed with ${code}`));
    };
    const ready = new Promise<void>((resolve, reject) => {
      this.#readyResolve = resolve;
      this.#readyReject = reject;
    });
    const hs1 = handshake.writeMessage(
      new TextEncoder().encode(JSON.stringify({ name: this.options.deviceName ?? "Test device", ua: "mock-desktop-test-client" })),
    );
    this.#socket.send(encodeFrame({ type: FrameType.HS1, channel: this.channel, deviceId: this.deviceId, body: hs1 }));
    await ready;
  }

  get rekeysSent(): number {
    return this.#crypto?.rekeysSent ?? 0;
  }
  get rekeysReceived(): number {
    return this.#crypto?.rekeysReceived ?? 0;
  }

  #failAll(error: Error): void {
    this.#readyReject?.(error);
    this.#readyReject = null;
    for (const pending of this.#pending.values()) pending.reject(error);
    this.#pending.clear();
  }

  #onFrame(bytes: Uint8Array): void {
    let frame: Frame;
    try {
      frame = decodeFrame(bytes);
    } catch {
      return;
    }
    if (frame.type === FrameType.CTRL) {
      this.ctrl.push(CtrlMessageSchema.parse(JSON.parse(new TextDecoder().decode(frame.body))));
      return;
    }
    // A device filters on its own channel (several tabs share a socket per device).
    if (frame.channel !== this.channel && frame.type !== FrameType.ERROR) return;
    if (frame.type === FrameType.ERROR) {
      if (frame.channel !== this.channel) return;
      const body = ErrorBodySchema.parse(JSON.parse(new TextDecoder().decode(frame.body)));
      this.#failAll(new ClientError(`relay error ${body.code}`, body.code));
      return;
    }
    if (frame.type === FrameType.BYE) {
      this.byeReceived = true;
      this.#failAll(new ClientError("BYE received", "bye"));
      return;
    }
    if (frame.type === FrameType.HS2) {
      try {
        const handshake = this.#handshake as HandshakeState;
        const payload = handshake.readMessage(frame.body);
        const proto = (JSON.parse(new TextDecoder().decode(payload)) as { proto?: number }).proto;
        if (proto !== 1) throw new ClientError(`unexpected proto ${String(proto)}`);
        // The crypto must exist before the next frame (the state message) is delivered.
        const { send, recv } = handshake.split();
        this.#crypto = new SecureChannel({
          send,
          recv,
          ...(this.options.rekeyAfterMessages === undefined ? {} : { rekeyAfterMessages: this.options.rekeyAfterMessages }),
          ...(this.options.rekeyAfterMs === undefined ? {} : { rekeyAfterMs: this.options.rekeyAfterMs }),
          ...(this.options.now ? { now: this.options.now } : {}),
        });
      } catch (error) {
        this.#failAll(error instanceof Error ? error : new Error(String(error)));
      }
      return;
    }
    if (frame.type === FrameType.TRANSPORT && this.#crypto) {
      try {
        this.#onTransport(frame);
      } catch (error) {
        // A frame that does not decrypt or parse ends the session (architecture.md 5.4).
        this.failure = error instanceof Error ? error : new Error(String(error));
        this.#failAll(this.failure);
      }
    }
  }

  #onTransport(frame: Frame): void {
    const result = (this.#crypto as SecureChannel).open(frame.body);
    if (result.type !== "message") return;
    const message = DesktopMessageSchema.parse(decodeEnvelopeJson(result.bytes));
    if ("type" in message && message.type === "state") {
      this.state = message;
      this.states.push(message);
      this.onState?.(message);
      this.#readyResolve?.();
      this.#readyResolve = null;
      this.#readyReject = null;
      return;
    }
    const response = message as EnvelopeResponse;
    const watch = this.#watches.get(response.rid);
    if (watch && response.ok) {
      if ("heartbeat" in response) {
        watch.heartbeats += 1;
        return;
      }
      if ("result" in response) {
        const frameResult = response.result as WatchFrame;
        watch.frames.push(frameResult);
        watch.onFrame?.(frameResult);
        return;
      }
    }
    const pending = this.#pending.get(response.rid);
    if (pending) {
      this.#pending.delete(response.rid);
      pending.resolve(response);
    }
  }

  #sendMessage(message: Parameters<typeof encodeEnvelope>[0]): void {
    if (!this.#crypto) throw new ClientError("not connected");
    for (const body of this.#crypto.seal(encodeEnvelope(message))) {
      this.#socket.send(encodeFrame({ type: FrameType.TRANSPORT, channel: this.channel, deviceId: this.deviceId, body }));
    }
  }

  #request(method: string, params: Record<string, unknown>, watch: boolean): { rid: number; response: Promise<EnvelopeResponse> } {
    const rid = this.#nextRid++;
    const request: EnvelopeRequest = {
      version: ENVELOPE_VERSION,
      rid,
      id: `cmd-${this.channel}-${rid}`,
      method,
      params,
      ...(watch ? { watch: true as const } : {}),
    };
    const response = new Promise<EnvelopeResponse>((resolve, reject) => this.#pending.set(rid, { resolve, reject }));
    this.#sendMessage(request);
    return { rid, response };
  }

  /** Sends a request and resolves with the raw response (`ok` or error). */
  async request(method: string, params: Record<string, unknown> = {}): Promise<EnvelopeResponse> {
    return this.#request(method, params, false).response;
  }

  /** Sends a request and returns `result`; throws `ClientError` with the desktop `code` on an error response. */
  async call<M extends MethodName>(method: M, params: ParamsOf<M> = {} as ParamsOf<M>): Promise<ResultOf<M>> {
    const response = await this.request(method, params as Record<string, unknown>);
    if (!response.ok) throw new ClientError(response.error, response.code);
    return (response as { result: ResultOf<M> }).result;
  }

  /** Opens `conversation.watch`; resolves after the `watching` ack. Frames collect in `frames`. */
  async openWatch(params: { conversationId: string; tail?: number; from?: string; incremental?: boolean }): Promise<ClientWatch> {
    const rid = this.#nextRid;
    const watch: ClientWatch = {
      rid,
      frames: [],
      heartbeats: 0,
      onFrame: null,
      cancel: () => {
        this.#sendMessage({ rid, cancel: true });
        this.#watches.delete(rid);
      },
    };
    this.#watches.set(rid, watch);
    const { response } = this.#request("conversation.watch", params, true);
    const ack = await response;
    if (!ack.ok) {
      this.#watches.delete(rid);
      throw new ClientError(ack.error, ack.code);
    }
    return watch;
  }

  /** Sends BYE and closes the session. */
  bye(): void {
    this.#socket.send(encodeFrame({ type: FrameType.BYE, channel: this.channel, deviceId: this.deviceId }));
  }

  /** Encrypts `plaintext` (an already-encoded kind byte and payload) and sends it. For tests of malformed plaintexts. */
  sendRawPlaintext(plaintext: Uint8Array): void {
    if (!this.#crypto) throw new ClientError("not connected");
    this.sendRawTransport(this.#crypto.sealRaw(plaintext));
  }

  /** Sends a TRANSPORT body as it is (tests that send garbage). */
  sendRawTransport(body: Uint8Array): void {
    this.#socket.send(encodeFrame({ type: FrameType.TRANSPORT, channel: this.channel, deviceId: this.deviceId, body }));
  }
}
