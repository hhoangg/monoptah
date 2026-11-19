import { describe, expect, it } from "vitest";
import {
  CloseCodes,
  FRAME_HEADER_SIZE,
  FrameType,
  MAX_FRAME_SIZE,
  decodeCtrlBody,
  decodeErrorBody,
  decodeFrame,
  encodeFrame,
  type Frame,
} from "@monoptah/protocol";
import { InMemoryRelay, connectRelay, type FrameSocket, type WebSocketLike } from "../src/index.ts";
import { DEVICE_ID, OTHER_DEVICE_ID } from "./harness.ts";

function recorder(socket: FrameSocket): { frames: Frame[]; texts: string[]; closes: number[] } {
  const rec = { frames: [] as Frame[], texts: [] as string[], closes: [] as number[] };
  socket.onmessage = (bytes) => rec.frames.push(decodeFrame(bytes));
  socket.ontext = (text) => rec.texts.push(text);
  socket.onclose = (code) => rec.closes.push(code);
  return rec;
}

const FORGED = "99999999-9999-4999-8999-999999999999";
const body = (n: number) => new Uint8Array(n).fill(0xab);

describe("in-memory relay: the rules of architecture.md 4.4", () => {
  it("overwrites the deviceId of device -> desktop frames with the authenticated id", async () => {
    const relay = new InMemoryRelay();
    const desktop = recorder(relay.connectDesktop());
    const device = relay.connectDevice(DEVICE_ID);
    device.send(encodeFrame({ type: FrameType.HS1, channel: 5, deviceId: FORGED, body: body(10) }));
    await relay.settled();
    const hs1 = desktop.frames.find((f) => f.type === FrameType.HS1);
    expect(hs1).toMatchObject({ channel: 5, deviceId: DEVICE_ID });
    expect(hs1?.body).toEqual(body(10));
  });

  it("a device may send only HS1, TRANSPORT and BYE: other types are dropped", async () => {
    const relay = new InMemoryRelay();
    const desktop = recorder(relay.connectDesktop());
    const device = relay.connectDevice(DEVICE_ID);
    for (const type of [FrameType.HS2, FrameType.CTRL, FrameType.ERROR]) device.send(encodeFrame({ type, channel: 1, deviceId: DEVICE_ID, body: body(2) }));
    const unknownType = encodeFrame({ type: FrameType.BYE, channel: 1, deviceId: DEVICE_ID });
    unknownType[1] = 0x42;
    device.send(unknownType);
    for (const type of [FrameType.HS1, FrameType.TRANSPORT, FrameType.BYE]) device.send(encodeFrame({ type, channel: 1, deviceId: DEVICE_ID, body: body(2) }));
    await relay.settled();
    expect(desktop.frames.filter((f) => f.type !== FrameType.CTRL).map((f) => f.type)).toEqual([FrameType.HS1, FrameType.TRANSPORT, FrameType.BYE]);
    expect(relay.dropped).toBe(4);
    expect(device.closed).toBe(false);
  });

  it("the desktop may send HS2, TRANSPORT and BYE; its CTRL and ERROR frames are dropped", async () => {
    const relay = new InMemoryRelay();
    const desktop = relay.connectDesktop();
    const device = recorder(relay.connectDevice(DEVICE_ID));
    for (const type of [FrameType.HS2, FrameType.TRANSPORT, FrameType.BYE, FrameType.CTRL, FrameType.ERROR]) {
      desktop.send(encodeFrame({ type, channel: 2, deviceId: DEVICE_ID, body: body(3) }));
    }
    await relay.settled();
    expect(device.frames.filter((f) => f.type !== FrameType.CTRL).map((f) => f.type)).toEqual([FrameType.HS2, FrameType.TRANSPORT, FrameType.BYE]);
  });

  it("routes desktop frames by header deviceId to every socket of that device only", async () => {
    const relay = new InMemoryRelay();
    const desktop = relay.connectDesktop();
    const tabA = recorder(relay.connectDevice(DEVICE_ID));
    const tabB = recorder(relay.connectDevice(DEVICE_ID));
    const other = recorder(relay.connectDevice(OTHER_DEVICE_ID));
    desktop.send(encodeFrame({ type: FrameType.TRANSPORT, channel: 9, deviceId: DEVICE_ID, body: body(4) }));
    await relay.settled();
    const transport = (r: ReturnType<typeof recorder>) => r.frames.filter((f) => f.type === FrameType.TRANSPORT);
    expect(transport(tabA)).toHaveLength(1);
    expect(transport(tabB)).toHaveLength(1);
    expect(transport(other)).toHaveLength(0);
  });

  it("answers a device frame with ERROR desktop_offline when no desktop is connected", async () => {
    const relay = new InMemoryRelay();
    const socket = relay.connectDevice(OTHER_DEVICE_ID);
    const rec = recorder(socket);
    socket.send(encodeFrame({ type: FrameType.HS1, channel: 6, deviceId: OTHER_DEVICE_ID, body: body(8) }));
    await relay.settled();
    const error = rec.frames.find((f) => f.type === FrameType.ERROR);
    expect(error).toMatchObject({ channel: 6, deviceId: OTHER_DEVICE_ID });
    expect(decodeErrorBody((error as Frame).body)).toEqual({ code: "desktop_offline" });
  });

  it("replies pong to ping and closes 4012 on any other text", async () => {
    const relay = new InMemoryRelay();
    const device = relay.connectDevice(DEVICE_ID);
    const rec = recorder(device);
    device.sendText("ping");
    await relay.settled();
    expect(rec.texts).toEqual(["pong"]);
    device.sendText("hello");
    await relay.settled();
    expect(rec.closes).toEqual([CloseCodes.PROTOCOL_ERROR]);
    expect(device.closed).toBe(true);
  });

  it("enforces the size bounds: 20 to 65,555 bytes, 1009 above and 4012 below", async () => {
    const relay = new InMemoryRelay();
    relay.connectDesktop();
    const ok = relay.connectDevice(DEVICE_ID);
    const okRec = recorder(ok);
    ok.send(encodeFrame({ type: FrameType.TRANSPORT, channel: 1, deviceId: DEVICE_ID, body: body(65_535) }));
    await relay.settled();
    expect(okRec.closes).toEqual([]);

    const big = relay.connectDevice(DEVICE_ID);
    const bigRec = recorder(big);
    big.send(new Uint8Array(MAX_FRAME_SIZE + 1).fill(1));
    await relay.settled();
    expect(bigRec.closes).toEqual([CloseCodes.MESSAGE_TOO_BIG]);

    const small = relay.connectDevice(DEVICE_ID);
    const smallRec = recorder(small);
    small.send(new Uint8Array(FRAME_HEADER_SIZE - 1));
    await relay.settled();
    expect(smallRec.closes).toEqual([CloseCodes.PROTOCOL_ERROR]);

    const wrongVersion = relay.connectDevice(DEVICE_ID);
    const versionRec = recorder(wrongVersion);
    const frame = encodeFrame({ type: FrameType.HS1, channel: 1, deviceId: DEVICE_ID });
    frame[0] = 2;
    wrongVersion.send(frame);
    await relay.settled();
    expect(versionRec.closes).toEqual([CloseCodes.PROTOCOL_ERROR]);
  });

  it("a new desktop socket replaces the old one, which closes 4009", async () => {
    const relay = new InMemoryRelay();
    const first = relay.connectDesktop();
    const firstRec = recorder(first);
    relay.connectDesktop();
    await relay.settled();
    expect(firstRec.closes).toEqual([CloseCodes.REPLACED]);
    expect(relay.desktopOnline).toBe(true);
  });

  it("closes a fifth socket of the same device with 4010", async () => {
    const relay = new InMemoryRelay();
    for (let i = 0; i < 4; i++) relay.connectDevice(DEVICE_ID);
    const fifth = relay.connectDevice(DEVICE_ID);
    const rec = recorder(fifth);
    await relay.settled();
    expect(rec.closes).toEqual([CloseCodes.ROOM_QUOTA]);
  });

  it("sends presence CTRL frames to devices and device CTRL frames to the desktop", async () => {
    const relay = new InMemoryRelay();
    const early = relay.connectDevice(DEVICE_ID);
    const earlyFrames: Frame[] = [];
    early.onmessage = (bytes) => earlyFrames.push(decodeFrame(bytes));
    await relay.settled();
    expect(decodeCtrlBody((earlyFrames[0] as Frame).body)).toEqual({ t: "presence", desktop: "offline" });

    const desktopSocket = relay.connectDesktop();
    const desktopFrames: Frame[] = [];
    desktopSocket.onmessage = (bytes) => desktopFrames.push(decodeFrame(bytes));
    await relay.settled();
    expect(decodeCtrlBody((earlyFrames[1] as Frame).body)).toEqual({ t: "presence", desktop: "online" });

    const late = relay.connectDevice(OTHER_DEVICE_ID);
    await relay.settled();
    expect(decodeCtrlBody((desktopFrames.at(-1) as Frame).body)).toEqual({ t: "device", id: OTHER_DEVICE_ID, event: "joined" });
    late.close();
    await relay.settled();
    expect(decodeCtrlBody((desktopFrames.at(-1) as Frame).body)).toEqual({ t: "device", id: OTHER_DEVICE_ID, event: "left" });

    desktopSocket.close();
    await relay.settled();
    expect(decodeCtrlBody((earlyFrames.at(-1) as Frame).body)).toEqual({ t: "presence", desktop: "offline" });
    expect(relay.desktopOnline).toBe(false);
  });

  it("revokeDevice closes that device's sockets with 4003", async () => {
    const relay = new InMemoryRelay();
    const rec = recorder(relay.connectDevice(DEVICE_ID));
    const other = recorder(relay.connectDevice(OTHER_DEVICE_ID));
    relay.revokeDevice(DEVICE_ID);
    await relay.settled();
    expect(rec.closes).toEqual([CloseCodes.REVOKED]);
    expect(other.closes).toEqual([]);
  });
});

class FakeWebSocket implements WebSocketLike {
  static last: FakeWebSocket | null = null;
  binaryType = "blob";
  sent: (Uint8Array | string)[] = [];
  closedWith: number | undefined;
  onopen: (() => void) | null = null;
  onerror: ((event: unknown) => void) | null = null;
  onclose: ((event: { code: number }) => void) | null = null;
  onmessage: ((event: { data: unknown }) => void) | null = null;
  readonly url: string;
  readonly init: unknown;
  constructor(url: string, init?: unknown) {
    this.url = url;
    this.init = init;
    FakeWebSocket.last = this;
  }
  send(data: Uint8Array | string): void {
    this.sent.push(data);
  }
  close(code?: number): void {
    this.closedWith = code;
  }
}

describe("WebSocket adapter shape", () => {
  it("opens with the URL and headers, asks for ArrayBuffers and resolves a FrameSocket once open", async () => {
    const promise = connectRelay("ws://localhost:8787/relay/desktop/abc", { Authorization: "Bearer example-token" }, { WebSocket: FakeWebSocket });
    const ws = FakeWebSocket.last as FakeWebSocket;
    expect(ws.url).toBe("ws://localhost:8787/relay/desktop/abc");
    expect(ws.init).toEqual({ headers: { Authorization: "Bearer example-token" } });
    expect(ws.binaryType).toBe("arraybuffer");
    ws.onopen?.();
    const socket = await promise;

    const frames: Uint8Array[] = [];
    const texts: string[] = [];
    const closes: number[] = [];
    socket.onmessage = (b) => frames.push(b);
    socket.ontext = (t) => texts.push(t);
    socket.onclose = (c) => closes.push(c);
    ws.onmessage?.({ data: Uint8Array.of(1, 2, 3).buffer });
    ws.onmessage?.({ data: "pong" });
    ws.onmessage?.({ data: Uint8Array.of(9, 8) });
    expect(frames).toEqual([Uint8Array.of(1, 2, 3), Uint8Array.of(9, 8)]);
    expect(texts).toEqual(["pong"]);

    socket.send(Uint8Array.of(7));
    socket.sendText("ping");
    expect(ws.sent).toEqual([Uint8Array.of(7), "ping"]);
    socket.close(4003);
    expect(ws.closedWith).toBe(4003);
    ws.onclose?.({ code: 4003 });
    expect(closes).toEqual([4003]);
    expect(socket.closed).toBe(true);
  });

  it("rejects when the socket errors or closes before opening", async () => {
    const failed = connectRelay("ws://localhost:1/relay", {}, { WebSocket: FakeWebSocket });
    (FakeWebSocket.last as FakeWebSocket).onerror?.({});
    await expect(failed).rejects.toThrow(/failed/);
    const closed = connectRelay("ws://localhost:1/relay", {}, { WebSocket: FakeWebSocket });
    (FakeWebSocket.last as FakeWebSocket).onclose?.({ code: 4001 });
    await expect(closed).rejects.toThrow(/4001/);
  });

  it("uses the global WebSocket by default and rejects cleanly when there is none", async () => {
    const original = globalThis.WebSocket;
    try {
      Object.defineProperty(globalThis, "WebSocket", { value: undefined, configurable: true, writable: true });
      await expect(connectRelay("ws://localhost:1/relay")).rejects.toThrow(/no global WebSocket/);
      Object.defineProperty(globalThis, "WebSocket", { value: FakeWebSocket, configurable: true, writable: true });
      const promise = connectRelay("ws://localhost:1/relay");
      (FakeWebSocket.last as FakeWebSocket).onopen?.();
      await expect(promise).resolves.toBeTruthy();
    } finally {
      Object.defineProperty(globalThis, "WebSocket", { value: original, configurable: true, writable: true });
    }
  });
});
