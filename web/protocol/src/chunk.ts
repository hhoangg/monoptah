// Plaintext layout inside a TRANSPORT body and chunk reassembly (architecture.md 5.2).
//
//   MSG   0x01 | utf-8 JSON envelope (the whole message fits one Noise message)
//   CHUNK 0x02 | msgId u32 BE | index u16 BE | flags u8 (bit0 = last) | bytes
//   REKEY 0x03 | (empty)
import {
  MAX_NOISE_PLAINTEXT,
  MAX_REASSEMBLY_IN_FLIGHT,
  MAX_REQUEST_BYTES,
  REASSEMBLY_TIMEOUT_MS,
} from "./limits.ts";

export const PlaintextKind = { MSG: 0x01, CHUNK: 0x02, REKEY: 0x03 } as const;
export type PlaintextKind = (typeof PlaintextKind)[keyof typeof PlaintextKind];

/** kind(1) + msgId(4) + index(2) + flags(1). */
export const CHUNK_HEADER_SIZE = 8;
/** Largest MSG payload: 65,519 minus the kind byte. */
export const MAX_MSG_PAYLOAD = MAX_NOISE_PLAINTEXT - 1;
/** Largest data bytes in one CHUNK. */
export const MAX_CHUNK_DATA = MAX_NOISE_PLAINTEXT - CHUNK_HEADER_SIZE;
/** `flags` bit0: this is the last chunk of the message. */
export const CHUNK_FLAG_LAST = 0x01;

export type PlaintextErrorKind = "empty" | "unknown_kind" | "too_short" | "too_long" | "rekey_not_empty";

export class PlaintextError extends Error {
  readonly kind: PlaintextErrorKind;
  constructor(kind: PlaintextErrorKind, message: string) {
    super(message);
    this.name = "PlaintextError";
    this.kind = kind;
  }
}

export type Plaintext =
  | { kind: "msg"; payload: Uint8Array }
  /** Flag bits other than `last` are ignored on decode (forward compatibility). */
  | { kind: "chunk"; msgId: number; index: number; last: boolean; data: Uint8Array }
  | { kind: "rekey" };

export function encodePlaintext(plaintext: Plaintext): Uint8Array {
  switch (plaintext.kind) {
    case "msg": {
      if (plaintext.payload.length > MAX_MSG_PAYLOAD) {
        throw new PlaintextError("too_long", `MSG payload of ${plaintext.payload.length} bytes exceeds ${MAX_MSG_PAYLOAD}`);
      }
      const out = new Uint8Array(1 + plaintext.payload.length);
      out[0] = PlaintextKind.MSG;
      out.set(plaintext.payload, 1);
      return out;
    }
    case "chunk": {
      if (plaintext.data.length > MAX_CHUNK_DATA) {
        throw new PlaintextError("too_long", `CHUNK data of ${plaintext.data.length} bytes exceeds ${MAX_CHUNK_DATA}`);
      }
      if (!Number.isInteger(plaintext.msgId) || plaintext.msgId < 0 || plaintext.msgId > 0xffff_ffff) {
        throw new PlaintextError("too_short", `msgId must be a u32, got ${plaintext.msgId}`);
      }
      if (!Number.isInteger(plaintext.index) || plaintext.index < 0 || plaintext.index > 0xffff) {
        throw new PlaintextError("too_short", `index must be a u16, got ${plaintext.index}`);
      }
      const out = new Uint8Array(CHUNK_HEADER_SIZE + plaintext.data.length);
      const view = new DataView(out.buffer);
      out[0] = PlaintextKind.CHUNK;
      view.setUint32(1, plaintext.msgId);
      view.setUint16(5, plaintext.index);
      out[7] = plaintext.last ? CHUNK_FLAG_LAST : 0;
      out.set(plaintext.data, CHUNK_HEADER_SIZE);
      return out;
    }
    case "rekey":
      return Uint8Array.of(PlaintextKind.REKEY);
  }
}

export function decodePlaintext(bytes: Uint8Array): Plaintext {
  if (bytes.length === 0) throw new PlaintextError("empty", "empty plaintext");
  if (bytes.length > MAX_NOISE_PLAINTEXT) {
    throw new PlaintextError("too_long", `plaintext of ${bytes.length} bytes exceeds ${MAX_NOISE_PLAINTEXT}`);
  }
  switch (bytes[0]) {
    case PlaintextKind.MSG:
      return { kind: "msg", payload: bytes.slice(1) };
    case PlaintextKind.CHUNK: {
      if (bytes.length < CHUNK_HEADER_SIZE) {
        throw new PlaintextError("too_short", `CHUNK of ${bytes.length} bytes is under ${CHUNK_HEADER_SIZE}`);
      }
      const view = new DataView(bytes.buffer, bytes.byteOffset, bytes.byteLength);
      return {
        kind: "chunk",
        msgId: view.getUint32(1),
        index: view.getUint16(5),
        last: ((bytes[7] as number) & CHUNK_FLAG_LAST) !== 0,
        data: bytes.slice(CHUNK_HEADER_SIZE),
      };
    }
    case PlaintextKind.REKEY:
      if (bytes.length !== 1) throw new PlaintextError("rekey_not_empty", "REKEY carries no payload");
      return { kind: "rekey" };
    default:
      throw new PlaintextError("unknown_kind", `unknown plaintext kind 0x${(bytes[0] as number).toString(16)}`);
  }
}

export type SplitErrorKind = "too_large";
export class SplitError extends Error {
  readonly kind: SplitErrorKind = "too_large";
  constructor(message: string) {
    super(message);
    this.name = "SplitError";
  }
}

/**
 * Splits one serialized message into encoded plaintexts, each at most 65,519
 * bytes: a single MSG when it fits, else CHUNKs with index 0.. and `last` on
 * the final one. Throws `SplitError` above `maxBytes` (default 8 MiB).
 */
export function splitMessage(bytes: Uint8Array, msgId: number, maxBytes: number = MAX_REQUEST_BYTES): Uint8Array[] {
  if (bytes.length > maxBytes) throw new SplitError(`message of ${bytes.length} bytes exceeds ${maxBytes}`);
  if (bytes.length <= MAX_MSG_PAYLOAD) return [encodePlaintext({ kind: "msg", payload: bytes })];
  const parts: Uint8Array[] = [];
  for (let offset = 0, index = 0; offset < bytes.length; offset += MAX_CHUNK_DATA, index++) {
    const end = Math.min(offset + MAX_CHUNK_DATA, bytes.length);
    parts.push(encodePlaintext({ kind: "chunk", msgId, index, last: end === bytes.length, data: bytes.subarray(offset, end) }));
  }
  return parts;
}

export type ReassemblyErrorKind = "too_large" | "too_many_in_flight" | "timeout" | "out_of_order";

/** Any of these ends the session with BYE (architecture.md 5.2, 5.4). */
export class ReassemblyError extends Error {
  readonly kind: ReassemblyErrorKind;
  constructor(kind: ReassemblyErrorKind, message: string) {
    super(message);
    this.name = "ReassemblyError";
    this.kind = kind;
  }
}

export type PushResult =
  | { type: "message"; bytes: Uint8Array }
  | { type: "partial" }
  | { type: "rekey" };

export interface ReassemblerOptions {
  /** Injectable clock in milliseconds (default `Date.now`). */
  now?: () => number;
  maxBytes?: number;
  maxInFlight?: number;
  timeoutMs?: number;
}

interface Pending {
  startedAt: number;
  nextIndex: number;
  size: number;
  parts: Uint8Array[];
}

/**
 * Collects CHUNKs by `msgId`: indexes must arrive in order from 0, at most 4
 * messages in flight, 8 MiB per message, 60 s from the first chunk to the last.
 * One instance per session and direction.
 */
export class Reassembler {
  readonly #now: () => number;
  readonly #maxBytes: number;
  readonly #maxInFlight: number;
  readonly #timeoutMs: number;
  readonly #pending = new Map<number, Pending>();

  constructor(options: ReassemblerOptions = {}) {
    this.#now = options.now ?? Date.now;
    this.#maxBytes = options.maxBytes ?? MAX_REQUEST_BYTES;
    this.#maxInFlight = options.maxInFlight ?? MAX_REASSEMBLY_IN_FLIGHT;
    this.#timeoutMs = options.timeoutMs ?? REASSEMBLY_TIMEOUT_MS;
  }

  /** Messages currently being reassembled. */
  get inFlight(): number {
    return this.#pending.size;
  }

  /** Throws `ReassemblyError("timeout")` if any message has been in flight longer than the limit. Call from a timer. */
  checkTimeouts(): void {
    const now = this.#now();
    for (const [msgId, pending] of this.#pending) {
      if (now - pending.startedAt > this.#timeoutMs) {
        throw new ReassemblyError("timeout", `message ${msgId} not complete after ${this.#timeoutMs} ms`);
      }
    }
  }

  /** Feeds one decoded-or-encoded plaintext. MSG completes at once, CHUNK may return `partial`. */
  push(plaintext: Uint8Array | Plaintext): PushResult {
    this.checkTimeouts();
    const decoded = plaintext instanceof Uint8Array ? decodePlaintext(plaintext) : plaintext;
    if (decoded.kind === "rekey") return { type: "rekey" };
    if (decoded.kind === "msg") return { type: "message", bytes: decoded.payload };

    let pending = this.#pending.get(decoded.msgId);
    if (!pending) {
      if (decoded.index !== 0) {
        throw new ReassemblyError("out_of_order", `message ${decoded.msgId} starts at index ${decoded.index}`);
      }
      if (this.#pending.size >= this.#maxInFlight) {
        throw new ReassemblyError("too_many_in_flight", `more than ${this.#maxInFlight} messages in reassembly`);
      }
      pending = { startedAt: this.#now(), nextIndex: 0, size: 0, parts: [] };
      this.#pending.set(decoded.msgId, pending);
    }
    if (decoded.index !== pending.nextIndex) {
      throw new ReassemblyError("out_of_order", `message ${decoded.msgId} expected index ${pending.nextIndex}, got ${decoded.index}`);
    }
    pending.size += decoded.data.length;
    if (pending.size > this.#maxBytes) {
      throw new ReassemblyError("too_large", `message ${decoded.msgId} exceeds ${this.#maxBytes} bytes`);
    }
    pending.parts.push(decoded.data);
    pending.nextIndex += 1;
    if (!decoded.last) return { type: "partial" };

    this.#pending.delete(decoded.msgId);
    const bytes = new Uint8Array(pending.size);
    let offset = 0;
    for (const part of pending.parts) {
      bytes.set(part, offset);
      offset += part.length;
    }
    return { type: "message", bytes };
  }

  /** Drops everything in flight (session closed). */
  reset(): void {
    this.#pending.clear();
  }
}
