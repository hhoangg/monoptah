// @vitest-environment node
import { afterEach, describe, expect, it, vi } from "vitest";
import { FrameType, PING_INTERVAL_MS, encodeFrame } from "@monoptah/protocol";
import {
  RelayConnectError,
  RelayConnection,
  openRelay,
  relayUrl,
  type RelaySocket,
  type WebSocketLike,
} from "./relay.ts";

afterEach(() => {
  vi.useRealTimers();
});

const DEVICE = "22222222-2222-4222-8222-222222222222";
const DESKTOP = "11111111-1111-4111-8111-111111111111";

class FakeSocket implements RelaySocket {
  onmessage: ((bytes: Uint8Array) => void) | null = null;
  ontext: ((text: string) => void) | null = null;
  onclose: ((code: number, reason?: string) => void) | null = null;
  closed = false;
  readonly binary: Uint8Array[] = [];
  readonly texts: string[] = [];
  closeCodes: (number | undefined)[] = [];
  send(bytes: Uint8Array): void {
    this.binary.push(bytes);
  }
  sendText(text: string): void {
    this.texts.push(text);
  }
  close(code?: number): void {
    this.closeCodes.push(code);
    this.closed = true;
    this.onclose?.(code ?? 1000, "");
  }
}

describe("RelayConnection", () => {
  it("sends the text ping every 30 seconds", () => {
    vi.useFakeTimers();
    const socket = new FakeSocket();
    new RelayConnection(socket);
    expect(PING_INTERVAL_MS).toBe(30_000);
    vi.advanceTimersByTime(29_999);
    expect(socket.texts).toEqual([]);
    vi.advanceTimersByTime(1);
    expect(socket.texts).toEqual(["ping"]);
    vi.advanceTimersByTime(60_000);
    expect(socket.texts).toEqual(["ping", "ping", "ping"]);
  });

  it("ignores pong and any other text, and stops pinging after close", () => {
    vi.useFakeTimers();
    const socket = new FakeSocket();
    const relay = new RelayConnection(socket);
    const frames: unknown[] = [];
    relay.onFrame = (f) => frames.push(f);
    socket.ontext?.("pong");
    socket.ontext?.("something else");
    expect(frames).toEqual([]);
    const closes: [number, string][] = [];
    relay.onClose = (code, reason) => closes.push([code, reason]);
    relay.close(1000);
    expect(closes).toEqual([[1000, ""]]);
    vi.advanceTimersByTime(120_000);
    expect(socket.texts).toEqual([]);
    expect(relay.closed).toBe(true);
  });

  it("delivers valid frames, drops invalid ones, and encodes sent frames", () => {
    const socket = new FakeSocket();
    const relay = new RelayConnection(socket);
    const frames: { type: number; channel: number; deviceId: string }[] = [];
    relay.onFrame = (f) => frames.push(f);
    socket.onmessage?.(encodeFrame({ type: FrameType.BYE, channel: 7, deviceId: DEVICE }));
    socket.onmessage?.(new Uint8Array(5));
    socket.onmessage?.(Uint8Array.of(9, ...new Array<number>(19).fill(0)));
    expect(frames).toEqual([{ type: FrameType.BYE, channel: 7, deviceId: DEVICE, body: new Uint8Array(0) }]);
    expect(relay.invalidFrames).toBe(2);
    relay.send({ type: FrameType.HS1, channel: 3, deviceId: DEVICE, body: Uint8Array.of(1, 2) });
    expect(socket.binary).toHaveLength(1);
    expect(socket.binary[0]?.length).toBe(22);
    relay.close();
    relay.send({ type: FrameType.BYE, channel: 3, deviceId: DEVICE });
    expect(socket.binary).toHaveLength(1);
  });

  it("reports a close from the other side with its code and reason, once", () => {
    const socket = new FakeSocket();
    const relay = new RelayConnection(socket);
    const closes: [number, string][] = [];
    relay.onClose = (code, reason) => closes.push([code, reason]);
    socket.onclose?.(4003, "revoked");
    socket.onclose?.(4003, "revoked");
    expect(closes).toEqual([[4003, "revoked"]]);
  });
});

describe("RelayConnection backlog (M4)", () => {
  const bye = (channel: number) => encodeFrame({ type: FrameType.BYE, channel, deviceId: DEVICE });

  it("delivers frames that arrived before a consumer attached, in order, then follows live frames", async () => {
    const socket = new FakeSocket();
    const relay = new RelayConnection(socket);
    socket.onmessage?.(bye(1));
    socket.onmessage?.(bye(2));
    const channels: number[] = [];
    relay.onFrame = (f) => channels.push(f.channel);
    // A frame that arrives right after attaching still comes after the backlog.
    socket.onmessage?.(bye(3));
    expect(channels).toEqual([]);
    await Promise.resolve();
    expect(channels).toEqual([1, 2, 3]);
    socket.onmessage?.(bye(4));
    expect(channels).toEqual([1, 2, 3, 4]);
  });

  it("drops frames after the consumer left", () => {
    const socket = new FakeSocket();
    const relay = new RelayConnection(socket);
    const channels: number[] = [];
    relay.onFrame = (f) => channels.push(f.channel);
    relay.onFrame = null;
    socket.onmessage?.(bye(1));
    relay.onFrame = (f) => channels.push(f.channel);
    expect(channels).toEqual([]);
  });
});

describe("relayUrl", () => {
  it("uses wss for https pages and ws for http://localhost", () => {
    const https = { protocol: "https:", host: "app.example.test", hostname: "app.example.test" };
    expect(relayUrl(DESKTOP, DEVICE, https)).toBe(`wss://app.example.test/relay/device/${DESKTOP}?device=${DEVICE}`);
    const dev = { protocol: "http:", host: "localhost:5173", hostname: "localhost" };
    expect(relayUrl(DESKTOP, DEVICE, dev)).toBe(`ws://localhost:5173/relay/device/${DESKTOP}?device=${DEVICE}`);
    // plain http on another host never downgrades to ws
    const other = { protocol: "http:", host: "example.test", hostname: "example.test" };
    expect(relayUrl(DESKTOP, DEVICE, other).startsWith("wss://")).toBe(true);
  });
});

class FakeWebSocket implements WebSocketLike {
  static last: FakeWebSocket | null = null;
  binaryType = "blob";
  onopen: (() => void) | null = null;
  onerror: ((event: unknown) => void) | null = null;
  onclose: ((event: { code: number; reason?: string }) => void) | null = null;
  onmessage: ((event: { data: unknown }) => void) | null = null;
  readonly sent: (Uint8Array | string)[] = [];
  closedWith: number | undefined;
  readonly url: string;
  constructor(url: string) {
    this.url = url;
    FakeWebSocket.last = this;
  }
  send(data: Uint8Array | string): void {
    this.sent.push(data);
  }
  close(code?: number): void {
    this.closedWith = code;
    this.onclose?.({ code: code ?? 1000, reason: "" });
  }
}

const location = { protocol: "https:", host: "app.example.test", hostname: "app.example.test" };

describe("openRelay", () => {
  it("opens the socket with an arraybuffer binaryType and wires frames, pings and close", async () => {
    vi.useFakeTimers();
    const promise = openRelay({ desktopId: DESKTOP, deviceId: DEVICE, WebSocket: FakeWebSocket, location });
    const ws = FakeWebSocket.last as FakeWebSocket;
    expect(ws.url).toBe(`wss://app.example.test/relay/device/${DESKTOP}?device=${DEVICE}`);
    expect(ws.binaryType).toBe("arraybuffer");
    ws.onopen?.();
    const relay = await promise;
    const types: number[] = [];
    relay.onFrame = (f) => types.push(f.type);
    const bytes = encodeFrame({ type: FrameType.BYE, channel: 1, deviceId: DEVICE });
    ws.onmessage?.({ data: bytes.buffer.slice(bytes.byteOffset, bytes.byteOffset + bytes.byteLength) });
    ws.onmessage?.({ data: "pong" });
    ws.onmessage?.({ data: bytes });
    expect(types).toEqual([FrameType.BYE, FrameType.BYE]);
    vi.advanceTimersByTime(PING_INTERVAL_MS);
    expect(ws.sent).toEqual(["ping"]);
    relay.send({ type: FrameType.BYE, channel: 1, deviceId: DEVICE });
    expect(ws.sent[1]).toBeInstanceOf(Uint8Array);
    const closes: number[] = [];
    relay.onClose = (code) => closes.push(code);
    ws.onclose?.({ code: 4003, reason: "revoked" });
    expect(closes).toEqual([4003]);
  });

  it("rejects with RelayConnectError when the socket closes before opening", async () => {
    const promise = openRelay({ desktopId: DESKTOP, deviceId: DEVICE, WebSocket: FakeWebSocket, location });
    FakeWebSocket.last?.onclose?.({ code: 1006 });
    await expect(promise).rejects.toBeInstanceOf(RelayConnectError);
    await expect(promise).rejects.toMatchObject({ closeCode: 1006 });
  });

  it("rejects and closes the socket when the upgrade does not complete in time (M3, L9)", async () => {
    vi.useFakeTimers();
    const promise = openRelay({ desktopId: DESKTOP, deviceId: DEVICE, WebSocket: FakeWebSocket, location, openTimeoutMs: 1000 });
    const outcome = promise.then(
      () => "opened",
      (error: unknown) => error,
    );
    const ws = FakeWebSocket.last as FakeWebSocket;
    await vi.advanceTimersByTimeAsync(999);
    expect(ws.closedWith).toBeUndefined();
    await vi.advanceTimersByTimeAsync(1);
    expect(await outcome).toBeInstanceOf(RelayConnectError);
    expect(ws.closedWith).toBe(1000);
  });

  it("defaults the open timeout to the 30 s handshake timeout", async () => {
    vi.useFakeTimers();
    const outcome = openRelay({ desktopId: DESKTOP, deviceId: DEVICE, WebSocket: FakeWebSocket, location }).then(
      () => "opened",
      () => "rejected",
    );
    await vi.advanceTimersByTimeAsync(29_999);
    expect(FakeWebSocket.last?.closedWith).toBeUndefined();
    await vi.advanceTimersByTimeAsync(1);
    expect(await outcome).toBe("rejected");
  });
});
