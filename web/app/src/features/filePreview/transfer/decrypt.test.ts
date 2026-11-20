import { describe, expect, it } from "vitest";
import { MockTransfers, type MemoryBackend, seal as nodeSeal } from "@monoptah/mock-desktop";
import { TRANSFER_CHUNK_BYTES } from "@monoptah/protocol";
import { chunkCount, decryptStream, importTransferKey, openChunk, sealForTest, sealedLength, sha256Hex, TransferCryptoError, type TransferKey } from "./decrypt";

const KEY = Uint8Array.from({ length: 32 }, (_, i) => i + 1);
const PREFIX = Uint8Array.from({ length: 8 }, (_, i) => 200 + i);
const b64 = (bytes: Uint8Array) => btoa(String.fromCharCode(...bytes));

async function transferKey(chunkBytes = TRANSFER_CHUNK_BYTES): Promise<TransferKey> {
  return importTransferKey(b64(KEY), b64(PREFIX), chunkBytes);
}

function pattern(length: number): Uint8Array {
  return Uint8Array.from({ length }, (_, i) => (i * 31) % 251);
}

/** A stream that delivers `bytes` in slices of `step` bytes. */
function streamOf(bytes: Uint8Array, step: number): ReadableStream<Uint8Array> {
  let at = 0;
  return new ReadableStream({
    pull(controller) {
      if (at >= bytes.length) return controller.close();
      controller.enqueue(bytes.slice(at, at + step));
      at += step;
    },
  });
}

describe("sizes", () => {
  it("counts the chunks and the sealed length like the desktop", () => {
    expect(chunkCount(0, 8)).toBe(1);
    expect(chunkCount(8, 8)).toBe(1);
    expect(chunkCount(9, 8)).toBe(2);
    expect(sealedLength(0, 8)).toBe(16);
    expect(sealedLength(9, 8)).toBe(9 + 32);
  });
});

describe("decryptStream", () => {
  it.each([0, 1, 7, 8, 9, 23, 24, 25, 100])("opens a file of %i bytes in chunks of 8 delivered in odd slices", async (size) => {
    const key = await transferKey(8);
    const plain = pattern(size);
    const sealed = await sealForTest(KEY, PREFIX, plain, 8);
    expect(sealed.length).toBe(sealedLength(size, 8));
    for (const step of [1, 5, 24, 1000]) {
      const received: number[] = [];
      const out = await decryptStream(key, streamOf(sealed, step), size, { onProgress: (n) => received.push(n) });
      expect(out).toEqual(plain);
      expect(received.at(-1) ?? sealed.length).toBe(sealed.length);
    }
  });

  it("opens what the desktop side seals with node:crypto (interop of the two implementations)", async () => {
    const plain = pattern(3 * TRANSFER_CHUNK_BYTES + 17);
    const sealed = nodeSeal(Buffer.from(KEY), Buffer.from(PREFIX), plain);
    const out = await decryptStream(await transferKey(), streamOf(new Uint8Array(sealed), 70_000), plain.length);
    expect(out).toEqual(plain);
    expect(await sha256Hex(out)).toMatch(/^[0-9a-f]{64}$/);
  });

  it("opens the fixed vector the Rust desktop produces", async () => {
    // Made by crates/monocode-core file_transfer tests (`the_web_vector_opens`): key 1..32, nonce prefix 200..207.
    const sealed = Uint8Array.from(atob(RUST_VECTOR.sealed), (c) => c.charCodeAt(0));
    const out = await decryptStream(await transferKey(), streamOf(sealed, 9), RUST_VECTOR.plain.length);
    expect(new TextDecoder().decode(out)).toBe(RUST_VECTOR.plain);
  });

  it("detects a dropped final chunk, a dropped first chunk, swapped chunks, a flipped bit and appended bytes", async () => {
    const key = await transferKey(8);
    const plain = pattern(30);
    const sealed = await sealForTest(KEY, PREFIX, plain, 8);
    const step = 8 + 16;
    const run = (bytes: Uint8Array, size = plain.length) => decryptStream(key, streamOf(bytes, 7), size);
    await expect(run(sealed.slice(0, sealed.length - 1))).rejects.toThrow(TransferCryptoError);
    await expect(run(sealed.slice(0, 3 * step))).rejects.toThrow(TransferCryptoError);
    await expect(run(sealed.slice(step))).rejects.toThrow(TransferCryptoError);
    const swapped = sealed.slice();
    swapped.set(sealed.slice(step, 2 * step), 0);
    swapped.set(sealed.slice(0, step), step);
    await expect(run(swapped)).rejects.toThrow(TransferCryptoError);
    const flipped = sealed.slice();
    flipped[step + 3] = (flipped[step + 3] ?? 0) ^ 1;
    await expect(run(flipped)).rejects.toThrow(TransferCryptoError);
    const longer = new Uint8Array(sealed.length + 1);
    longer.set(sealed);
    await expect(run(longer)).rejects.toThrow("longer");
    // The announced size must match: a bigger claim reads the stream as short.
    await expect(run(sealed, plain.length + 1)).rejects.toThrow(TransferCryptoError);
  });

  it("refuses a wrong key and a chunk used as final or at another index", async () => {
    const plain = pattern(10);
    const sealed = await sealForTest(KEY, PREFIX, plain, 8);
    const other = await importTransferKey(b64(KEY.map((b) => b ^ 1)), b64(PREFIX), 8);
    await expect(decryptStream(other, streamOf(sealed, 50), 10)).rejects.toThrow(TransferCryptoError);
    const key = await transferKey(8);
    const first = sealed.slice(0, 24);
    await expect(openChunk(key, 0, true, first)).rejects.toThrow(TransferCryptoError);
    await expect(openChunk(key, 1, false, first)).rejects.toThrow(TransferCryptoError);
    expect(await openChunk(key, 0, false, first)).toEqual(plain.slice(0, 8));
  });

  it("stops when aborted", async () => {
    const key = await transferKey(8);
    const sealed = await sealForTest(KEY, PREFIX, pattern(100), 8);
    const controller = new AbortController();
    controller.abort();
    await expect(decryptStream(key, streamOf(sealed, 10), 100, { signal: controller.signal })).rejects.toThrow();
  });
});

describe("the key", () => {
  it("must be 32 key bytes and 8 nonce bytes, and a positive chunk size", async () => {
    await expect(importTransferKey(b64(KEY.slice(0, 31)), b64(PREFIX), 8)).rejects.toThrow(TransferCryptoError);
    await expect(importTransferKey(b64(KEY), b64(PREFIX.slice(0, 7)), 8)).rejects.toThrow(TransferCryptoError);
    await expect(importTransferKey(b64(KEY), b64(PREFIX), 0)).rejects.toThrow(TransferCryptoError);
    await expect(importTransferKey("%%%not base64", b64(PREFIX), 8)).rejects.toThrow(TransferCryptoError);
  });

  it("is not extractable from the browser's crypto", async () => {
    const key = await transferKey();
    expect(key.key.extractable).toBe(false);
  });
});

describe("mock desktop sealing", () => {
  it("is consistent with the browser for a prepared file", async () => {
    const transfers = new MockTransfers();
    const backend = transfers.backend as MemoryBackend;
    const prepared = await transfers.prepare({ path: "report.html" }, false);
    const sealed = backend.objects.get(prepared.object)!;
    const key = await importTransferKey(prepared.key, prepared.nonce, prepared.chunkBytes);
    const out = await decryptStream(key, streamOf(sealed, 33), prepared.size);
    expect(await sha256Hex(out)).toBe(prepared.hash);
  });
});

// The Rust side prints this vector in `file_transfer::tests::the_web_vector_opens`; both sides must open it.
const RUST_VECTOR = { plain: "Hello from the desktop, sealed in R2.\n", sealed: "cfviAsesskP7d6Fggrd6l4QMavUXjLw4b+jUas5ZJy9SLC+GDzt6IDzx6wk5MIxIXUywDxjm" };
