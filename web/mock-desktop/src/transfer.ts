// The desktop half of `file.prepare` for tests: the same steps as crates/monocode-core/src/mobile/file_transfer
// (hash the content, name the object `<hash>.<keyId>`, ask the server, seal and upload when it lacks the object, keep the
// key by hash) over fixture files. Uses the noble libraries only: this package also runs in the browser (the dev preview
// of the app), where node:crypto does not exist.
import { gcm } from "@noble/ciphers/aes.js";
import { hmac } from "@noble/hashes/hmac.js";
import { sha256 } from "@noble/hashes/sha2.js";
import { bytesToHex } from "@noble/hashes/utils.js";
import type { ResultOf } from "@monoptah/protocol";
import { TRANSFER_CHUNK_BYTES, TRANSFER_TAG_BYTES } from "@monoptah/protocol";
import { transferContent, MentionError, type FixtureContent } from "./mentioned.ts";
import { FixtureError } from "./fixtures.ts";

/** The server's object store as the desktop sees it (HEAD and PUT of `/api/desktop/transfers/:object`). */
export interface TransferBackend {
  head(object: string): Promise<number | null>;
  put(object: string, sealed: Uint8Array): Promise<void>;
}

interface Kept {
  key: Uint8Array;
  noncePrefix: Uint8Array;
}

function random(length: number): Uint8Array {
  return crypto.getRandomValues(new Uint8Array(length));
}

function base64(bytes: Uint8Array): string {
  let binary = "";
  for (const byte of bytes) binary += String.fromCharCode(byte);
  return btoa(binary);
}

/** Seals `plain` the way the desktop does (see decrypt.ts in the app for the format). */
export function seal(key: Uint8Array, noncePrefix: Uint8Array, plain: Uint8Array, chunkBytes = TRANSFER_CHUNK_BYTES): Uint8Array {
  // Copies: a caller from another realm (a jsdom test passing a node Buffer) would fail noble's instanceof checks.
  key = Uint8Array.from(key);
  noncePrefix = Uint8Array.from(noncePrefix);
  plain = Uint8Array.from(plain);
  const count = Math.max(1, Math.ceil(plain.length / chunkBytes));
  const out = new Uint8Array(sealedSize(plain.length, chunkBytes));
  let at = 0;
  for (let index = 0; index < count; index++) {
    const nonce = new Uint8Array(12);
    nonce.set(noncePrefix, 0);
    new DataView(nonce.buffer).setUint32(8, index, false);
    const aad = new Uint8Array(8);
    aad.set([0x6d, 0x74, 0x31], 0);
    new DataView(aad.buffer).setUint32(3, index, false);
    aad[7] = index === count - 1 ? 1 : 0;
    const piece = plain.subarray(index * chunkBytes, Math.min(plain.length, (index + 1) * chunkBytes));
    // The sealed chunk is the ciphertext followed by its 16 byte tag.
    const sealed = gcm(key, nonce, aad).encrypt(piece);
    out.set(sealed, at);
    at += sealed.length;
  }
  return out;
}

export function sealedSize(plain: number, chunkBytes = TRANSFER_CHUNK_BYTES): number {
  return plain + Math.max(1, Math.ceil(plain / chunkBytes)) * TRANSFER_TAG_BYTES;
}

/** A server that keeps objects in memory (the default, for tests that do not run a Worker). */
export class MemoryBackend implements TransferBackend {
  readonly objects = new Map<string, Uint8Array>();
  head(object: string): Promise<number | null> {
    return Promise.resolve(this.objects.get(object)?.length ?? null);
  }
  put(object: string, sealed: Uint8Array): Promise<void> {
    if (!this.objects.has(object)) this.objects.set(object, sealed);
    return Promise.resolve();
  }
}

export class MockTransfers {
  /** Keys by content hash, as the desktop's key store keeps them. */
  readonly keys = new Map<string, Kept>();
  /** The secret of this desktop that turns a content hash into an object name (the real desktop keeps it with its keys). */
  readonly nameSecret: Uint8Array = random(32);
  /** Uploads made (the object names), for tests. */
  readonly uploads: string[] = [];
  /** Calls answered, for tests. */
  prepares = 0;
  /** The server the objects go to: in memory until a test sets a real one. */
  backend: TransferBackend = new MemoryBackend();

  /** `file.prepare` for a device. Async because the upload is. */
  async prepare(params: { path: string; asset?: string | undefined; source?: boolean | undefined }, control: boolean): Promise<ResultOf<"file.prepare">> {
    this.prepares++;
    let file: FixtureContent;
    try {
      file = transferContent(params, control);
    } catch (error) {
      if (error instanceof MentionError) throw new FixtureError(error.message, error.code);
      throw error;
    }
    const hash = bytesToHex(sha256(file.bytes));
    let kept = this.keys.get(hash);
    if (!kept) {
      kept = { key: random(32), noncePrefix: random(8) };
      this.keys.set(hash, kept);
    }
    const keyId = bytesToHex(sha256(new Uint8Array([...new TextEncoder().encode("monoptah-transfer-key-id"), ...kept.key]))).slice(0, 16);
    const object = `${bytesToHex(hmac(sha256, this.nameSecret, new TextEncoder().encode(hash)))}.${keyId}`;
    const present = await this.backend.head(object);
    const reused = present === sealedSize(file.bytes.length);
    if (!reused) {
      await this.backend.put(object, seal(kept.key, kept.noncePrefix, file.bytes));
      this.uploads.push(object);
    }
    return {
      path: file.path,
      name: file.name,
      kind: file.kind,
      mime: file.mime,
      size: file.bytes.length,
      hash,
      object,
      key: base64(kept.key),
      nonce: base64(kept.noncePrefix),
      chunkBytes: TRANSFER_CHUNK_BYTES,
      reused,
      ...(file.assets ? { assets: file.assets } : {}),
      ...(file.project ? { project: file.project } : {}),
    };
  }
}
