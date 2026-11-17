import { describe, expect, it } from "vitest";
import {
  FRAME_HEADER_SIZE,
  FrameError,
  FrameType,
  MAX_FRAME_BODY,
  MAX_FRAME_SIZE,
  MIN_FRAME_SIZE,
  RELAY_RULES,
  bytesToUuid,
  decodeCtrlBody,
  decodeErrorBody,
  decodeFrame,
  deviceMaySend,
  desktopMaySend,
  encodeCtrlBody,
  encodeErrorBody,
  encodeFrame,
  frameTypeName,
  overwriteDeviceId,
  uuidToBytes,
  type FrameType as FrameTypeValue,
} from "../src/index.ts";
import { expandBytes, vectors } from "./helpers.ts";

const DEVICE = "0b9a4f3e-7c1d-4e8a-9b2f-3d5c6e7f8a90";

function kindOf(fn: () => unknown): string | undefined {
  try {
    fn();
  } catch (error) {
    if (error instanceof FrameError) return error.kind;
    throw error;
  }
  return undefined;
}

describe("frame vectors", () => {
  it("has a vector for every frame type", () => {
    const covered = new Set(vectors.frames.valid.map((v) => v.decoded.type));
    expect([...covered].sort()).toEqual(Object.keys(FrameType).sort());
  });

  for (const vector of vectors.frames.valid) {
    it(`round-trips: ${vector.name}`, () => {
      const bytes = expandBytes(vector.frame);
      const decoded = decodeFrame(bytes);
      expect(frameTypeName(decoded.type)).toBe(vector.decoded.type);
      expect(decoded.type).toBe(vector.decoded.typeCode);
      expect(decoded.channel).toBe(vector.decoded.channel);
      expect(decoded.deviceId).toBe(vector.decoded.deviceId);
      expect(decoded.body).toEqual(expandBytes(vector.decoded.body));
      const encoded = encodeFrame({
        type: FrameType[vector.decoded.type as keyof typeof FrameType],
        channel: vector.decoded.channel,
        deviceId: vector.decoded.deviceId,
        body: expandBytes(vector.decoded.body),
      });
      expect(encoded).toEqual(bytes);
    });
  }

  for (const vector of vectors.frames.invalid) {
    it(`rejects: ${vector.name}`, () => {
      expect(kindOf(() => decodeFrame(expandBytes(vector.frame)))).toBe(vector.error);
    });
  }
});

describe("frame size bounds", () => {
  const frameOf = (bodyLength: number) =>
    encodeFrame({ type: FrameType.TRANSPORT, channel: 1, deviceId: DEVICE, body: new Uint8Array(bodyLength) });

  it("accepts 20 to 65,555 bytes", () => {
    expect(MIN_FRAME_SIZE).toBe(20);
    expect(MAX_FRAME_SIZE).toBe(65_555);
    expect(decodeFrame(frameOf(0)).body.length).toBe(0);
    expect(frameOf(0).length).toBe(20);
    expect(decodeFrame(frameOf(MAX_FRAME_BODY)).body.length).toBe(65_535);
    expect(frameOf(MAX_FRAME_BODY).length).toBe(65_555);
  });

  it("rejects 19 and 65,556 bytes", () => {
    expect(kindOf(() => decodeFrame(new Uint8Array(19)))).toBe("too_short");
    const big = frameOf(MAX_FRAME_BODY);
    expect(kindOf(() => decodeFrame(new Uint8Array(65_556).fill(1)))).toBe("too_long");
    expect(big.length + 1).toBe(65_556);
  });

  it("refuses to encode a body over 65,535", () => {
    expect(kindOf(() => frameOf(MAX_FRAME_BODY + 1))).toBe("too_long");
  });
});

describe("frame encode validation", () => {
  it("rejects a bad uuid", () => {
    expect(kindOf(() => encodeFrame({ type: FrameType.BYE, channel: 0, deviceId: "not-a-uuid" }))).toBe("bad_uuid");
    expect(kindOf(() => encodeFrame({ type: FrameType.BYE, channel: 0, deviceId: `${DEVICE}0` }))).toBe("bad_uuid");
  });
  it("rejects a bad channel and an unknown type", () => {
    expect(kindOf(() => encodeFrame({ type: FrameType.BYE, channel: 65_536, deviceId: DEVICE }))).toBe("bad_channel");
    expect(kindOf(() => encodeFrame({ type: FrameType.BYE, channel: -1, deviceId: DEVICE }))).toBe("bad_channel");
    expect(kindOf(() => encodeFrame({ type: 0x77 as FrameTypeValue, channel: 0, deviceId: DEVICE }))).toBe("unknown_type");
  });
  it("writes the header fields at the documented offsets", () => {
    const bytes = encodeFrame({ type: FrameType.CTRL, channel: 0x0102, deviceId: DEVICE, body: Uint8Array.of(9) });
    expect(Array.from(bytes.subarray(0, 4))).toEqual([1, 0x10, 0x01, 0x02]);
    expect(bytesToUuid(bytes.subarray(4, FRAME_HEADER_SIZE))).toBe(DEVICE);
    expect(bytes[20]).toBe(9);
  });
});

describe("uuid helpers", () => {
  it("round-trips, accepting upper case on input", () => {
    const bytes = uuidToBytes(DEVICE.toUpperCase());
    expect(bytes.length).toBe(16);
    expect(bytesToUuid(bytes)).toBe(DEVICE);
  });
  it("needs 16 bytes", () => {
    expect(kindOf(() => bytesToUuid(new Uint8Array(15)))).toBe("bad_uuid");
  });
});

describe("relay rules", () => {
  it("lets a device send HS1, TRANSPORT and BYE only", () => {
    expect(RELAY_RULES.deviceMaySend).toEqual([FrameType.HS1, FrameType.TRANSPORT, FrameType.BYE]);
    expect(deviceMaySend(FrameType.HS1)).toBe(true);
    expect(deviceMaySend(FrameType.HS2)).toBe(false);
    expect(deviceMaySend(FrameType.CTRL)).toBe(false);
    expect(deviceMaySend(FrameType.ERROR)).toBe(false);
    expect(desktopMaySend(FrameType.HS2)).toBe(true);
    expect(desktopMaySend(FrameType.HS1)).toBe(false);
  });
  it("overwrites the deviceId header and nothing else", () => {
    const original = encodeFrame({ type: FrameType.TRANSPORT, channel: 5, deviceId: "ffffffff-ffff-4fff-8fff-ffffffffffff", body: Uint8Array.of(1, 2) });
    const stamped = overwriteDeviceId(original, DEVICE);
    const decoded = decodeFrame(stamped);
    expect(decoded.deviceId).toBe(DEVICE);
    expect(decoded.channel).toBe(5);
    expect(Array.from(decoded.body)).toEqual([1, 2]);
    expect(decodeFrame(original).deviceId).toBe("ffffffff-ffff-4fff-8fff-ffffffffffff");
  });
});

describe("CTRL and ERROR bodies", () => {
  it("round-trips the three control messages", () => {
    for (const message of [
      { t: "presence", desktop: "online" },
      { t: "presence", desktop: "offline" },
      { t: "device", id: DEVICE, event: "joined" },
      { t: "device", id: DEVICE, event: "left" },
      { t: "grants" },
    ] as const) {
      expect(decodeCtrlBody(encodeCtrlBody(message))).toEqual(message);
    }
  });
  it("rejects an unknown control message", () => {
    expect(() => decodeCtrlBody(new TextEncoder().encode('{"t":"nope"}'))).toThrow();
    expect(() => decodeCtrlBody(new TextEncoder().encode('{"t":"presence","desktop":"asleep"}'))).toThrow();
  });
  it("round-trips an ERROR body", () => {
    expect(decodeErrorBody(encodeErrorBody({ code: "desktop_offline" }))).toEqual({ code: "desktop_offline" });
    expect(() => decodeErrorBody(new TextEncoder().encode("{}"))).toThrow();
  });
});
