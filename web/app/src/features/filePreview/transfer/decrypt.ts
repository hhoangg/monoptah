// Opening an encrypted file transfer (crates/monocode-core/src/mobile/file_transfer/crypto.rs, web/docs/security.md
// "File transfers"). The object is AES-256-GCM in independent chunks:
//   chunk i: plaintext `chunkBytes` long (the last one shorter, or exactly full; an empty file is one empty chunk),
//   nonce:   8 byte prefix (from the desktop) || i as a big-endian u32,
//   aad:     "mt1" || i as a big-endian u32 || 1 byte (1 for the final chunk),
// so a reordered, dropped, appended or altered chunk fails its tag. The key and the prefix come over the encrypted relay
// channel only. Nothing here touches the DOM or the network.
import { TRANSFER_TAG_BYTES } from "@monoptah/protocol";

export class TransferCryptoError extends Error {
  constructor(message: string) {
    super(message);
    this.name = "TransferCryptoError";
  }
}

export interface TransferKey {
  key: CryptoKey;
  noncePrefix: Uint8Array;
  chunkBytes: number;
}

function fromBase64(text: string): Uint8Array {
  try {
    const binary = atob(text);
    return Uint8Array.from(binary, (char) => char.charCodeAt(0));
  } catch {
    throw new TransferCryptoError("The desktop sent a damaged key");
  }
}

/** Imports the key of a `file.prepare` answer. The key is not extractable. */
export async function importTransferKey(keyBase64: string, nonceBase64: string, chunkBytes: number): Promise<TransferKey> {
  const raw = fromBase64(keyBase64);
  const noncePrefix = fromBase64(nonceBase64);
  if (raw.length !== 32 || noncePrefix.length !== 8 || !Number.isInteger(chunkBytes) || chunkBytes <= 0) {
    throw new TransferCryptoError("The desktop sent a damaged key");
  }
  const key = await crypto.subtle.importKey("raw", raw as BufferSource, { name: "AES-GCM" }, false, ["decrypt"]);
  raw.fill(0);
  return { key, noncePrefix, chunkBytes };
}

/** Chunks of a file of `plain` bytes (at least one). */
export function chunkCount(plain: number, chunkBytes: number): number {
  return Math.max(1, Math.ceil(plain / chunkBytes));
}

/** The size of the stored object for a file of `plain` bytes. */
export function sealedLength(plain: number, chunkBytes: number): number {
  return plain + chunkCount(plain, chunkBytes) * TRANSFER_TAG_BYTES;
}

function nonceOf(prefix: Uint8Array, index: number): Uint8Array {
  const nonce = new Uint8Array(12);
  nonce.set(prefix, 0);
  new DataView(nonce.buffer).setUint32(8, index, false);
  return nonce;
}

function aadOf(index: number, last: boolean): Uint8Array {
  const aad = new Uint8Array(8);
  aad.set([0x6d, 0x74, 0x31], 0);
  new DataView(aad.buffer).setUint32(3, index, false);
  aad[7] = last ? 1 : 0;
  return aad;
}

/** Opens one sealed chunk. Throws `TransferCryptoError` when it was altered, moved or is not the final chunk it claims to be. */
export async function openChunk(transfer: TransferKey, index: number, last: boolean, sealed: Uint8Array): Promise<Uint8Array> {
  try {
    const plain = await crypto.subtle.decrypt(
      { name: "AES-GCM", iv: nonceOf(transfer.noncePrefix, index) as BufferSource, additionalData: aadOf(index, last) as BufferSource, tagLength: 128 },
      transfer.key,
      sealed as BufferSource,
    );
    return new Uint8Array(plain);
  } catch {
    throw new TransferCryptoError("The file could not be decrypted");
  }
}

/** Seals a whole file the way the desktop does (tests and tools; the browser only opens). */
export async function sealForTest(keyBytes: Uint8Array, noncePrefix: Uint8Array, plain: Uint8Array, chunkBytes: number): Promise<Uint8Array> {
  const key = await crypto.subtle.importKey("raw", keyBytes as BufferSource, { name: "AES-GCM" }, false, ["encrypt"]);
  const count = chunkCount(plain.length, chunkBytes);
  const parts: Uint8Array[] = [];
  for (let index = 0; index < count; index++) {
    const piece = plain.subarray(index * chunkBytes, Math.min(plain.length, (index + 1) * chunkBytes));
    const sealed = await crypto.subtle.encrypt({ name: "AES-GCM", iv: nonceOf(noncePrefix, index) as BufferSource, additionalData: aadOf(index, index === count - 1) as BufferSource, tagLength: 128 }, key, piece as BufferSource);
    parts.push(new Uint8Array(sealed));
  }
  const out = new Uint8Array(sealedLength(plain.length, chunkBytes));
  let at = 0;
  for (const part of parts) {
    out.set(part, at);
    at += part.length;
  }
  return out;
}

export interface DecryptOptions {
  signal?: AbortSignal;
  /** Called with the sealed bytes received so far and the total. */
  onProgress?: (received: number, total: number) => void;
}

/**
 * Reads `body` to its end and opens every chunk as soon as it is complete, so memory holds the plaintext and one
 * chunk, and large files show progress. The stream must be exactly the sealed size of `size` plain bytes: a short or
 * long stream is an error, never a partial file.
 */
export async function decryptStream(transfer: TransferKey, body: ReadableStream<Uint8Array>, size: number, options: DecryptOptions = {}): Promise<Uint8Array> {
  const { chunkBytes } = transfer;
  const step = chunkBytes + TRANSFER_TAG_BYTES;
  const count = chunkCount(size, chunkBytes);
  const total = sealedLength(size, chunkBytes);
  const out = new Uint8Array(size);
  let pending = new Uint8Array(0);
  let received = 0;
  let index = 0;
  let written = 0;
  const reader = body.getReader();
  const take = async (): Promise<void> => {
    for (;;) {
      const last = index === count - 1;
      const need = last ? size - index * chunkBytes + TRANSFER_TAG_BYTES : step;
      if (pending.length < need) return;
      const plain = await openChunk(transfer, index, last, pending.subarray(0, need));
      out.set(plain, written);
      written += plain.length;
      pending = pending.slice(need);
      index++;
      if (index === count) return;
    }
  };
  try {
    for (;;) {
      options.signal?.throwIfAborted();
      const { done, value } = await reader.read();
      if (done) break;
      received += value.length;
      if (received > total) throw new TransferCryptoError("The file is longer than announced");
      const joined = new Uint8Array(pending.length + value.length);
      joined.set(pending, 0);
      joined.set(value, pending.length);
      pending = joined;
      await take();
      options.onProgress?.(received, total);
    }
  } finally {
    reader.releaseLock();
  }
  if (received !== total || index !== count || written !== size) throw new TransferCryptoError("The file is shorter than announced");
  return out;
}

/** SHA-256 of `bytes` as lowercase hex. */
export async function sha256Hex(bytes: Uint8Array): Promise<string> {
  const digest = new Uint8Array(await crypto.subtle.digest("SHA-256", bytes as BufferSource));
  return Array.from(digest, (byte) => byte.toString(16).padStart(2, "0")).join("");
}
