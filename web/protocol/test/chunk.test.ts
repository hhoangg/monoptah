import { describe, expect, it } from "vitest";
import {
  CHUNK_HEADER_SIZE,
  MAX_CHUNK_DATA,
  MAX_MSG_PAYLOAD,
  MAX_NOISE_PLAINTEXT,
  MAX_REQUEST_BYTES,
  PlaintextError,
  Reassembler,
  ReassemblyError,
  SplitError,
  decodePlaintext,
  encodePlaintext,
  splitMessage,
} from "../src/index.ts";
import { expandBytes, vectors } from "./helpers.ts";

function patterned(length: number): Uint8Array {
  const out = new Uint8Array(length);
  for (let i = 0; i < length; i++) out[i] = (i * 31 + 7) & 0xff;
  return out;
}

function plaintextKind(fn: () => unknown): string | undefined {
  try {
    fn();
  } catch (error) {
    if (error instanceof PlaintextError) return error.kind;
    throw error;
  }
  return undefined;
}

function reassemblyKind(fn: () => unknown): string | undefined {
  try {
    fn();
  } catch (error) {
    if (error instanceof ReassemblyError) return error.kind;
    throw error;
  }
  return undefined;
}

describe("plaintext vectors", () => {
  for (const vector of vectors.plaintext.valid) {
    it(`round-trips: ${vector.name}`, () => {
      const bytes = expandBytes(vector.plaintext);
      const decoded = decodePlaintext(bytes);
      expect(decoded.kind).toBe(vector.decoded.kind);
      if (decoded.kind === "msg") {
        expect(decoded.payload).toEqual(expandBytes(vector.decoded.payload as never));
      } else if (decoded.kind === "chunk") {
        expect(decoded.msgId).toBe(vector.decoded.msgId);
        expect(decoded.index).toBe(vector.decoded.index);
        expect(decoded.last).toBe(vector.decoded.last);
        expect(decoded.data).toEqual(expandBytes(vector.decoded.data as never));
      }
      expect(encodePlaintext(decoded)).toEqual(bytes);
    });
  }
  for (const vector of vectors.plaintext.invalid) {
    it(`rejects: ${vector.name}`, () => {
      expect(plaintextKind(() => decodePlaintext(expandBytes(vector.plaintext)))).toBe(vector.error);
    });
  }
  for (const vector of vectors.plaintext.split) {
    it(`splits as the vector says: ${vector.name}`, () => {
      const message = new Uint8Array(vector.messageLength).fill(Buffer.from(vector.messageFill, "hex")[0] as number);
      const parts = splitMessage(message, vector.msgId);
      expect(parts.map((part) => part.length)).toEqual(vector.partLengths);
    });
  }
});

describe("splitMessage", () => {
  it("uses one MSG up to 65,518 bytes and chunks above", () => {
    expect(splitMessage(new Uint8Array(MAX_MSG_PAYLOAD), 1)).toHaveLength(1);
    expect(decodePlaintext(splitMessage(new Uint8Array(MAX_MSG_PAYLOAD), 1)[0] as Uint8Array).kind).toBe("msg");
    const two = splitMessage(new Uint8Array(MAX_MSG_PAYLOAD + 1), 1);
    expect(two).toHaveLength(2);
    expect(decodePlaintext(two[0] as Uint8Array).kind).toBe("chunk");
  });

  it("keeps every plaintext within 65,519 bytes and flags only the last chunk", () => {
    const parts = splitMessage(patterned(300_000), 9);
    for (const part of parts) expect(part.length).toBeLessThanOrEqual(MAX_NOISE_PLAINTEXT);
    const decoded = parts.map((part) => decodePlaintext(part));
    decoded.forEach((chunk, i) => {
      expect(chunk.kind).toBe("chunk");
      if (chunk.kind === "chunk") {
        expect(chunk.index).toBe(i);
        expect(chunk.msgId).toBe(9);
        expect(chunk.last).toBe(i === decoded.length - 1);
      }
    });
    expect(parts[0]?.length).toBe(MAX_CHUNK_DATA + CHUNK_HEADER_SIZE);
  });

  it("refuses more than 8 MiB", () => {
    expect(() => splitMessage(new Uint8Array(MAX_REQUEST_BYTES + 1), 1)).toThrow(SplitError);
    expect(splitMessage(new Uint8Array(MAX_REQUEST_BYTES), 1).length).toBeGreaterThan(100);
  });
});

describe("Reassembler", () => {
  it("returns a MSG at once", () => {
    const reassembler = new Reassembler();
    const message = patterned(10);
    const result = reassembler.push(splitMessage(message, 1)[0] as Uint8Array);
    expect(result).toEqual({ type: "message", bytes: message });
  });

  it("reassembles a chunked message byte for byte", () => {
    const reassembler = new Reassembler();
    const message = patterned(200_000);
    const parts = splitMessage(message, 77);
    let last;
    for (const part of parts) last = reassembler.push(part);
    expect(last).toEqual({ type: "message", bytes: message });
    expect(reassembler.inFlight).toBe(0);
  });

  it("reassembles exactly 8 MiB and rejects one byte more", () => {
    const reassembler = new Reassembler();
    const message = patterned(MAX_REQUEST_BYTES);
    let last;
    for (const part of splitMessage(message, 1)) last = reassembler.push(part);
    expect(last).toMatchObject({ type: "message" });
    expect(last && last.type === "message" ? last.bytes.length : 0).toBe(MAX_REQUEST_BYTES);

    const over = new Reassembler();
    const parts = splitMessage(patterned(MAX_REQUEST_BYTES + 1), 2, MAX_REQUEST_BYTES + 1);
    expect(
      reassemblyKind(() => {
        for (const part of parts) over.push(part);
      }),
    ).toBe("too_large");
  });

  it("passes REKEY through", () => {
    expect(new Reassembler().push(encodePlaintext({ kind: "rekey" }))).toEqual({ type: "rekey" });
  });

  it("rejects out-of-order and duplicate indexes", () => {
    const parts = splitMessage(patterned(200_000), 5);
    const skip = new Reassembler();
    skip.push(parts[0] as Uint8Array);
    expect(reassemblyKind(() => skip.push(parts[2] as Uint8Array))).toBe("out_of_order");

    const dup = new Reassembler();
    dup.push(parts[0] as Uint8Array);
    expect(reassemblyKind(() => dup.push(parts[0] as Uint8Array))).toBe("out_of_order");

    const unknownStart = new Reassembler();
    expect(reassemblyKind(() => unknownStart.push(parts[1] as Uint8Array))).toBe("out_of_order");
  });

  it("allows 4 messages in flight and rejects a fifth", () => {
    const reassembler = new Reassembler();
    for (let id = 1; id <= 4; id++) reassembler.push((splitMessage(patterned(100_000), id)[0]) as Uint8Array);
    expect(reassembler.inFlight).toBe(4);
    expect(reassemblyKind(() => reassembler.push(splitMessage(patterned(100_000), 5)[0] as Uint8Array))).toBe("too_many_in_flight");
    // A message that completes frees its slot.
    const parts = splitMessage(patterned(100_000), 1);
    reassembler.push(parts[1] as Uint8Array);
    expect(reassembler.inFlight).toBe(3);
    reassembler.push(splitMessage(patterned(100_000), 6)[0] as Uint8Array);
    expect(reassembler.inFlight).toBe(4);
  });

  it("times out 60 s after the first chunk, on the injected clock", () => {
    let now = 1_000;
    const reassembler = new Reassembler({ now: () => now });
    const parts = splitMessage(patterned(200_000), 3);
    reassembler.push(parts[0] as Uint8Array);
    now += 60_000; // exactly 60 s is still within the limit
    expect(() => reassembler.checkTimeouts()).not.toThrow();
    reassembler.push(parts[1] as Uint8Array);
    now += 1;
    expect(reassemblyKind(() => reassembler.checkTimeouts())).toBe("timeout");
    expect(reassemblyKind(() => reassembler.push(parts[2] as Uint8Array))).toBe("timeout");
  });

  it("does not time out a message that finishes in time", () => {
    let now = 0;
    const reassembler = new Reassembler({ now: () => now });
    const message = patterned(200_000);
    let last;
    for (const part of splitMessage(message, 3)) {
      last = reassembler.push(part);
      now += 10_000;
    }
    expect(last).toEqual({ type: "message", bytes: message });
  });

  it("reset drops everything in flight", () => {
    const reassembler = new Reassembler();
    reassembler.push(splitMessage(patterned(100_000), 1)[0] as Uint8Array);
    reassembler.reset();
    expect(reassembler.inFlight).toBe(0);
  });
});
