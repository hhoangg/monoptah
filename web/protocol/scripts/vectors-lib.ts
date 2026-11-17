// Builds vectors.json and methods.json from the protocol source. Used by
// gen-vectors.ts (writes the files) and by the staleness test (compares them).
// Never edit the generated files by hand: change the source and run
// `pnpm --filter @monoptah/protocol vectors:gen`.
import {
  API_ERROR_CODES,
  CLOSE_CODES,
  DESKTOP_ERROR_CODES,
  FRAME_HEADER_SIZE,
  FRAME_VERSION,
  FrameType,
  KNOWN_CAPABILITIES,
  MAX_CHUNK_DATA,
  MAX_FRAME_BODY,
  MAX_FRAME_SIZE,
  MAX_MSG_PAYLOAD,
  MAX_NOISE_PLAINTEXT,
  MAX_REQUEST_BYTES,
  METHOD_LIST,
  MIN_FRAME_SIZE,
  PROTOCOL_VERSION,
  RELAY_ERROR_CODES,
  RELAY_RULES,
  encodeFrame,
  encodePlaintext,
  frameTypeName,
  splitMessage,
  type Plaintext,
} from "../src/index.ts";

/**
 * A byte string in vectors.json: either `{"hex": "..."}` or
 * `{"prefix": "<hex>", "fill": "<one byte hex>", "length": N}`, which means the
 * prefix followed by the fill byte repeated until the string is N bytes long.
 */
export type BytesVector = { hex: string } | { prefix: string; fill: string; length: number };

export function expandBytes(vector: BytesVector): Uint8Array {
  if ("hex" in vector) return new Uint8Array(Buffer.from(vector.hex, "hex"));
  const out = new Uint8Array(vector.length);
  const prefix = Buffer.from(vector.prefix, "hex");
  out.set(prefix);
  out.fill(Buffer.from(vector.fill, "hex")[0] as number, prefix.length);
  return out;
}

function hex(bytes: Uint8Array): string {
  return Buffer.from(bytes).toString("hex");
}

function fixed(length: number, fill: number, prefix: Uint8Array = new Uint8Array(0)): Uint8Array {
  const out = new Uint8Array(length);
  out.set(prefix);
  out.fill(fill, prefix.length);
  return out;
}

/** Hex when short, else prefix plus fill. `bytes` must be `prefix` followed by `fill` bytes. */
function compactOrHex(bytes: Uint8Array, prefixLength: number): BytesVector {
  if (bytes.length <= 256) return { hex: hex(bytes) };
  const fill = bytes[prefixLength] as number;
  return { prefix: hex(bytes.subarray(0, prefixLength)), fill: hex(Uint8Array.of(fill)), length: bytes.length };
}

const DEVICE_ID = "0b9a4f3e-7c1d-4e8a-9b2f-3d5c6e7f8a90";
const OTHER_DEVICE_ID = "ffffffff-0000-4000-8000-123456789abc";
const textEncoder = new TextEncoder();
const json = (value: unknown) => textEncoder.encode(JSON.stringify(value));

type FrameTypeNameKey = keyof typeof FrameType;

interface ValidFrameInput {
  name: string;
  type: FrameTypeNameKey;
  channel: number;
  deviceId: string;
  body: Uint8Array;
  /** Where the fill starts inside the body, for compact output. */
  bodyPrefixLength?: number;
}

function validFrame(input: ValidFrameInput) {
  const frame = encodeFrame({ type: FrameType[input.type], channel: input.channel, deviceId: input.deviceId, body: input.body });
  const prefixLength = input.bodyPrefixLength ?? 0;
  return {
    name: input.name,
    frame: compactOrHex(frame, FRAME_HEADER_SIZE + prefixLength),
    decoded: {
      version: FRAME_VERSION,
      type: input.type,
      typeCode: FrameType[input.type],
      channel: input.channel,
      deviceId: input.deviceId,
      body: compactOrHex(input.body, prefixLength),
    },
  };
}

function header(version: number, type: number, channel = 7, deviceId = DEVICE_ID): Uint8Array {
  const frame = encodeFrame({ type: FrameType.BYE, channel, deviceId });
  frame[0] = version;
  frame[1] = type;
  return frame;
}

function buildFrameVectors() {
  const valid = [
    validFrame({ name: "hs1 with a 48-byte body", type: "HS1", channel: 1, deviceId: DEVICE_ID, body: fixed(48, 0x11) }),
    validFrame({ name: "hs2 with a 48-byte body", type: "HS2", channel: 1, deviceId: DEVICE_ID, body: fixed(48, 0x22) }),
    validFrame({ name: "transport with a short body", type: "TRANSPORT", channel: 258, deviceId: DEVICE_ID, body: Uint8Array.of(1, 2, 3, 4, 5) }),
    validFrame({ name: "bye with an empty body (minimum size 20)", type: "BYE", channel: 0, deviceId: DEVICE_ID, body: new Uint8Array(0) }),
    validFrame({ name: "ctrl presence online", type: "CTRL", channel: 0, deviceId: DEVICE_ID, body: json({ t: "presence", desktop: "online" }) }),
    validFrame({ name: "ctrl presence offline", type: "CTRL", channel: 0, deviceId: DEVICE_ID, body: json({ t: "presence", desktop: "offline" }) }),
    validFrame({ name: "ctrl device joined", type: "CTRL", channel: 0, deviceId: OTHER_DEVICE_ID, body: json({ t: "device", id: OTHER_DEVICE_ID, event: "joined" }) }),
    validFrame({ name: "ctrl device left", type: "CTRL", channel: 0, deviceId: OTHER_DEVICE_ID, body: json({ t: "device", id: OTHER_DEVICE_ID, event: "left" }) }),
    validFrame({ name: "ctrl grants", type: "CTRL", channel: 0, deviceId: DEVICE_ID, body: json({ t: "grants" }) }),
    validFrame({ name: "error desktop_offline", type: "ERROR", channel: 3, deviceId: DEVICE_ID, body: json({ code: "desktop_offline" }) }),
    validFrame({ name: "transport on channel 65535", type: "TRANSPORT", channel: 0xffff, deviceId: OTHER_DEVICE_ID, body: fixed(100, 0x5a) }),
    validFrame({
      name: "transport at the maximum size 65,555 (65,535 byte body)",
      type: "TRANSPORT",
      channel: 9,
      deviceId: DEVICE_ID,
      body: fixed(MAX_FRAME_BODY, 0xab),
    }),
  ];

  const invalid = [
    { name: "unknown version 2", error: "unknown_version", frame: { hex: hex(header(2, FrameType.TRANSPORT)) } },
    { name: "unknown version 0", error: "unknown_version", frame: { hex: hex(header(0, FrameType.TRANSPORT)) } },
    { name: "unknown type 0x05", error: "unknown_type", frame: { hex: hex(header(FRAME_VERSION, 0x05)) } },
    { name: "unknown type 0x00", error: "unknown_type", frame: { hex: hex(header(FRAME_VERSION, 0x00)) } },
    { name: "unknown type 0x12", error: "unknown_type", frame: { hex: hex(header(FRAME_VERSION, 0x12)) } },
    { name: "empty frame", error: "too_short", frame: { hex: "" } },
    { name: "19 bytes (one under the minimum)", error: "too_short", frame: { prefix: "0103", fill: "00", length: MIN_FRAME_SIZE - 1 } },
    {
      name: "65,556 bytes (one over the maximum)",
      error: "too_long",
      frame: { prefix: hex(header(FRAME_VERSION, FrameType.TRANSPORT)), fill: "ab", length: MAX_FRAME_SIZE + 1 },
    },
  ];
  return { valid, invalid };
}

function validPlaintext(name: string, plaintext: Plaintext) {
  const bytes = encodePlaintext(plaintext);
  const decoded =
    plaintext.kind === "msg"
      ? { kind: "msg", payload: compactOrHex(plaintext.payload, 0) }
      : plaintext.kind === "chunk"
        ? { kind: "chunk", msgId: plaintext.msgId, index: plaintext.index, last: plaintext.last, data: compactOrHex(plaintext.data, 0) }
        : { kind: "rekey" };
  return { name, plaintext: compactOrHex(bytes, plaintext.kind === "chunk" ? 8 : 1), decoded };
}

function buildPlaintextVectors() {
  const valid = [
    validPlaintext("msg with a JSON envelope", { kind: "msg", payload: json({ version: 1, rid: 7, id: "c1", method: "workspace.list", params: {} }) }),
    validPlaintext("msg with an empty payload", { kind: "msg", payload: new Uint8Array(0) }),
    validPlaintext("msg at the maximum payload (65,518 bytes)", { kind: "msg", payload: fixed(MAX_MSG_PAYLOAD, 0x61) }),
    validPlaintext("chunk first", { kind: "chunk", msgId: 0x01020304, index: 0, last: false, data: Uint8Array.of(1, 2, 3, 4) }),
    validPlaintext("chunk middle", { kind: "chunk", msgId: 0x01020304, index: 1, last: false, data: Uint8Array.of(5, 6, 7, 8) }),
    validPlaintext("chunk last", { kind: "chunk", msgId: 0x01020304, index: 2, last: true, data: Uint8Array.of(9, 10) }),
    validPlaintext("chunk last with no data", { kind: "chunk", msgId: 0xffffffff, index: 0xffff, last: true, data: new Uint8Array(0) }),
    validPlaintext("chunk at the maximum data (65,511 bytes)", { kind: "chunk", msgId: 42, index: 3, last: false, data: fixed(MAX_CHUNK_DATA, 0x62) }),
    validPlaintext("rekey", { kind: "rekey" }),
  ];
  const invalid = [
    { name: "empty plaintext", error: "empty", plaintext: { hex: "" } },
    { name: "unknown kind 0x04", error: "unknown_kind", plaintext: { hex: "0400" } },
    { name: "unknown kind 0x00", error: "unknown_kind", plaintext: { hex: "00" } },
    { name: "chunk shorter than its 8 byte header", error: "too_short", plaintext: { hex: "0200000001" } },
    { name: "rekey with a payload", error: "rekey_not_empty", plaintext: { hex: "0300" } },
    { name: "65,520 bytes (one over the maximum)", error: "too_long", plaintext: { prefix: "01", fill: "ab", length: MAX_NOISE_PLAINTEXT + 1 } },
  ];

  // splitMessage: the message is `messageFill` repeated `messageLength` times. One part is a
  // MSG; more are CHUNKs with index 0.. and the last flag on the final part. `partLengths` are
  // the plaintext lengths, header included.
  const split = [0, 1, 65_518, 65_519, 65_511 * 2 + 8 + 1, MAX_REQUEST_BYTES].map((messageLength) => {
    const msgId = 0x0a0b0c0d;
    const parts = splitMessage(fixed(messageLength, 0x7e), msgId);
    return {
      name: `split of a ${messageLength} byte message`,
      messageLength,
      messageFill: "7e",
      msgId,
      partLengths: parts.map((part) => part.length),
    };
  });
  return { valid, invalid, split };
}

function sortedUnique(values: readonly string[]): string[] {
  return [...new Set(values)].sort();
}

export function buildVectors() {
  return {
    $comment: [
      "Generated by web/protocol/scripts/gen-vectors.ts from web/protocol/src. Never edit by hand.",
      "Run: pnpm --filter @monoptah/protocol vectors:gen. Both the TypeScript and the Rust test suites read this file.",
      "Byte strings are {\"hex\": \"...\"} or {\"prefix\": \"<hex>\", \"fill\": \"<one byte hex>\", \"length\": N}: the prefix, then the fill byte repeated up to N bytes total.",
      "frames.valid[]: {name, frame, decoded}. encodeFrame(decoded) must equal frame and decodeFrame(frame) must equal decoded. decoded = {version, type (name), typeCode, channel (u16), deviceId (lowercase uuid), body}.",
      "frames.invalid[]: {name, error, frame}. Decoding must fail with that error. error is one of unknown_version, unknown_type, too_short, too_long. Check order: length, version, type.",
      "plaintext.valid[]: {name, plaintext, decoded}. decoded.kind is msg {payload}, chunk {msgId (u32), index (u16), last, data} or rekey. Layout: MSG 0x01|payload; CHUNK 0x02|msgId u32 BE|index u16 BE|flags u8 (bit0 = last)|data; REKEY 0x03.",
      "plaintext.invalid[]: {name, error, plaintext}. error is one of empty, unknown_kind, too_short, too_long, rekey_not_empty.",
      "plaintext.split[]: splitting `messageLength` bytes of `messageFill` with `msgId` yields plaintexts of `partLengths` bytes (one part is a MSG, more are CHUNKs from index 0, `last` on the final one).",
      "close[]: relay close codes with the reaction of each side (architecture.md 4.5). errors: the `code` lists. constants: values both sides must agree on.",
      "state: {capabilities, messages[]}. capabilities is the list of names a desktop advertises today; messages are example `state` messages (pending, confirmed, revoked) that must parse with the state schema.",
      "Noise handshake vectors are not part of this file yet (spike S-04): the `noise` key is absent on purpose.",
    ].join("\n"),
    version: 1,
    constants: {
      protocolVersion: PROTOCOL_VERSION,
      frameVersion: FRAME_VERSION,
      frameHeaderSize: FRAME_HEADER_SIZE,
      minFrameSize: MIN_FRAME_SIZE,
      maxFrameBody: MAX_FRAME_BODY,
      maxFrameSize: MAX_FRAME_SIZE,
      maxNoisePlaintext: MAX_NOISE_PLAINTEXT,
      maxMsgPayload: MAX_MSG_PAYLOAD,
      maxChunkData: MAX_CHUNK_DATA,
      maxReassembledBytes: MAX_REQUEST_BYTES,
      frameTypes: Object.fromEntries(Object.entries(FrameType)),
      relay: {
        deviceMaySend: RELAY_RULES.deviceMaySend.map((type) => frameTypeName(type)),
        desktopMaySend: RELAY_RULES.desktopMaySend.map((type) => frameTypeName(type)),
        relayOnly: RELAY_RULES.relayOnly.map((type) => frameTypeName(type)),
      },
    },
    frames: buildFrameVectors(),
    plaintext: buildPlaintextVectors(),
    close: CLOSE_CODES,
    state: {
      capabilities: [...KNOWN_CAPABILITIES],
      messages: [
        { type: "state", state: "pending" },
        {
          type: "state",
          state: "confirmed",
          permission: "control",
          proto: PROTOCOL_VERSION,
          capabilities: [...KNOWN_CAPABILITIES],
          desktop: { name: "Example Desktop", version: "1.2.3" },
        },
        { type: "state", state: "revoked" },
      ],
    },
    errors: {
      desktop: sortedUnique(DESKTOP_ERROR_CODES),
      relay: sortedUnique(RELAY_ERROR_CODES),
      api: sortedUnique(API_ERROR_CODES),
    },
  };
}

export function buildMethodsJson() {
  return {
    version: 1,
    methods: METHOD_LIST.map(({ name, def }) => ({ name, class: def.class, transport: def.transport })),
  };
}

export function serialize(value: unknown): string {
  return `${JSON.stringify(value, null, 2)}\n`;
}
