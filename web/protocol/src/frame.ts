// Outer relay frame codec (architecture.md 5.1) and the relay rules as data (4.4).
//
//   offset size field
//   0      1    version   = 1
//   1      1    type      HS1 0x01, HS2 0x02, TRANSPORT 0x03, BYE 0x04, CTRL 0x10, ERROR 0x11
//   2      2    channel   u16 big-endian
//   4      16   deviceId  UUID bytes (the relay overwrites it on device -> desktop frames)
//   20     N    body      N <= 65,535
import "./zod-config.ts";
import { z } from "zod";
import { FRAME_HEADER_SIZE, FRAME_VERSION, MAX_FRAME_BODY, MAX_FRAME_SIZE, MIN_FRAME_SIZE } from "./limits.ts";

export const FrameType = {
  HS1: 0x01,
  HS2: 0x02,
  TRANSPORT: 0x03,
  BYE: 0x04,
  CTRL: 0x10,
  ERROR: 0x11,
} as const;
export type FrameType = (typeof FrameType)[keyof typeof FrameType];
export type FrameTypeName = keyof typeof FrameType;

const FRAME_TYPE_VALUES: ReadonlySet<number> = new Set(Object.values(FrameType));
const FRAME_TYPE_NAMES = Object.fromEntries(Object.entries(FrameType).map(([name, value]) => [value, name])) as Record<
  number,
  FrameTypeName
>;

export function frameTypeName(type: number): FrameTypeName | undefined {
  return FRAME_TYPE_NAMES[type];
}

export type FrameErrorKind =
  | "unknown_version"
  | "unknown_type"
  | "too_short"
  | "too_long"
  | "bad_uuid"
  | "bad_channel";

/** Thrown by `decodeFrame` and `encodeFrame`. `kind` is stable and listed in vectors.json. */
export class FrameError extends Error {
  readonly kind: FrameErrorKind;
  constructor(kind: FrameErrorKind, message: string) {
    super(message);
    this.name = "FrameError";
    this.kind = kind;
  }
}

export interface Frame {
  type: FrameType;
  /** u16, chosen by the device per tab or session. */
  channel: number;
  /** Canonical lowercase UUID string. */
  deviceId: string;
  body: Uint8Array;
}

// --- UUID helpers ---

const UUID_RE = /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/i;

/** Parses a UUID string (any case) into 16 bytes. Throws `FrameError("bad_uuid")`. */
export function uuidToBytes(uuid: string): Uint8Array {
  if (!UUID_RE.test(uuid)) throw new FrameError("bad_uuid", `not a UUID: ${JSON.stringify(uuid)}`);
  const hex = uuid.replaceAll("-", "");
  const out = new Uint8Array(16);
  for (let i = 0; i < 16; i++) out[i] = Number.parseInt(hex.slice(i * 2, i * 2 + 2), 16);
  return out;
}

/** Formats 16 bytes as a lowercase UUID string. Throws `FrameError("bad_uuid")` for another length. */
export function bytesToUuid(bytes: Uint8Array): string {
  if (bytes.length !== 16) throw new FrameError("bad_uuid", `UUID needs 16 bytes, got ${bytes.length}`);
  const hex = Array.from(bytes, (b) => b.toString(16).padStart(2, "0")).join("");
  return `${hex.slice(0, 8)}-${hex.slice(8, 12)}-${hex.slice(12, 16)}-${hex.slice(16, 20)}-${hex.slice(20)}`;
}

// --- Codec ---

export function encodeFrame(frame: { type: FrameType; channel: number; deviceId: string; body?: Uint8Array }): Uint8Array {
  if (!FRAME_TYPE_VALUES.has(frame.type)) throw new FrameError("unknown_type", `unknown frame type ${frame.type}`);
  if (!Number.isInteger(frame.channel) || frame.channel < 0 || frame.channel > 0xffff) {
    throw new FrameError("bad_channel", `channel must be a u16, got ${frame.channel}`);
  }
  const body = frame.body ?? new Uint8Array(0);
  if (body.length > MAX_FRAME_BODY) throw new FrameError("too_long", `body of ${body.length} bytes exceeds ${MAX_FRAME_BODY}`);
  const out = new Uint8Array(FRAME_HEADER_SIZE + body.length);
  out[0] = FRAME_VERSION;
  out[1] = frame.type;
  out[2] = frame.channel >> 8;
  out[3] = frame.channel & 0xff;
  out.set(uuidToBytes(frame.deviceId), 4);
  out.set(body, FRAME_HEADER_SIZE);
  return out;
}

/** Decodes a frame. Check order: length, version, type. Throws `FrameError`. */
export function decodeFrame(bytes: Uint8Array): Frame {
  if (bytes.length < MIN_FRAME_SIZE) throw new FrameError("too_short", `frame of ${bytes.length} bytes is under ${MIN_FRAME_SIZE}`);
  if (bytes.length > MAX_FRAME_SIZE) throw new FrameError("too_long", `frame of ${bytes.length} bytes exceeds ${MAX_FRAME_SIZE}`);
  const version = bytes[0] as number;
  if (version !== FRAME_VERSION) throw new FrameError("unknown_version", `unknown frame version ${version}`);
  const type = bytes[1] as number;
  if (!FRAME_TYPE_VALUES.has(type)) throw new FrameError("unknown_type", `unknown frame type 0x${type.toString(16)}`);
  return {
    type: type as FrameType,
    channel: ((bytes[2] as number) << 8) | (bytes[3] as number),
    deviceId: bytesToUuid(bytes.subarray(4, FRAME_HEADER_SIZE)),
    body: bytes.slice(FRAME_HEADER_SIZE),
  };
}

/**
 * What the relay does on device -> desktop frames: copy of `bytes` with the
 * deviceId header field replaced by the authenticated id (architecture.md 4.4).
 * Caller has already validated the frame length and type.
 */
export function overwriteDeviceId(bytes: Uint8Array, deviceId: string): Uint8Array {
  if (bytes.length < MIN_FRAME_SIZE) throw new FrameError("too_short", `frame of ${bytes.length} bytes is under ${MIN_FRAME_SIZE}`);
  const out = bytes.slice();
  out.set(uuidToBytes(deviceId), 4);
  return out;
}

// --- Relay rules as data (architecture.md 4.4, 5.1) ---

export const RELAY_RULES = {
  /** A device may send these; anything else is dropped by the DO. */
  deviceMaySend: [FrameType.HS1, FrameType.TRANSPORT, FrameType.BYE],
  /** The desktop sends these (HS2 and BYE are routed by the deviceId in the header). */
  desktopMaySend: [FrameType.HS2, FrameType.TRANSPORT, FrameType.BYE],
  /** Only the relay originates these. */
  relayOnly: [FrameType.CTRL, FrameType.ERROR],
  /** Device -> desktop: the relay overwrites the deviceId header field with the authenticated id. */
  overwritesDeviceIdOnDeviceFrames: true,
  /** Only binary messages are relayed; text other than `ping` is a protocol error (close 4012). */
  binaryOnly: true,
} as const satisfies {
  deviceMaySend: readonly FrameType[];
  desktopMaySend: readonly FrameType[];
  relayOnly: readonly FrameType[];
  overwritesDeviceIdOnDeviceFrames: boolean;
  binaryOnly: boolean;
};

export function deviceMaySend(type: number): boolean {
  return (RELAY_RULES.deviceMaySend as readonly number[]).includes(type);
}
export function desktopMaySend(type: number): boolean {
  return (RELAY_RULES.desktopMaySend as readonly number[]).includes(type);
}

// --- CTRL and ERROR bodies (relay -> peer, JSON, metadata only; 4.4) ---

export const CtrlMessageSchema = z.discriminatedUnion("t", [
  z.object({ t: z.literal("presence"), desktop: z.enum(["online", "offline"]) }),
  z.object({ t: z.literal("device"), id: z.string(), event: z.enum(["joined", "left"]) }),
  z.object({ t: z.literal("grants") }),
]);
export type CtrlMessage = z.infer<typeof CtrlMessageSchema>;

/** ERROR body `{code}` (for example `desktop_offline`). See `RELAY_ERROR_CODES` in errors.ts. */
export const ErrorBodySchema = z.object({ code: z.string() });
export type ErrorBody = z.infer<typeof ErrorBodySchema>;

const textEncoder = new TextEncoder();
const textDecoder = new TextDecoder("utf-8", { fatal: true, ignoreBOM: false });

export function encodeCtrlBody(message: CtrlMessage): Uint8Array {
  return textEncoder.encode(JSON.stringify(CtrlMessageSchema.parse(message)));
}
export function decodeCtrlBody(body: Uint8Array): CtrlMessage {
  return CtrlMessageSchema.parse(JSON.parse(textDecoder.decode(body)));
}
export function encodeErrorBody(body: ErrorBody): Uint8Array {
  return textEncoder.encode(JSON.stringify(ErrorBodySchema.parse(body)));
}
export function decodeErrorBody(body: Uint8Array): ErrorBody {
  return ErrorBodySchema.parse(JSON.parse(textDecoder.decode(body)));
}
