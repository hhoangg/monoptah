// The desktop half of `file.prepare` for tests: the same steps as crates/monocode-core/src/mobile/file_transfer
// (hash the content, name the object `<hash>.<keyId>`, ask the server, seal and upload when it lacks the object, keep the
// key by hash) over fixture files. Uses node:crypto so the handler stays synchronous up to the upload.
import { createCipheriv, createHash, createHmac, randomBytes } from "node:crypto";
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
  key: Buffer;
  noncePrefix: Buffer;
}

/** Seals `plain` the way the desktop does (see decrypt.ts in the app for the format). */
export function seal(key: Buffer, noncePrefix: Buffer, plain: Uint8Array, chunkBytes = TRANSFER_CHUNK_BYTES): Buffer {
  const count = Math.max(1, Math.ceil(plain.length / chunkBytes));
  const parts: Buffer[] = [];
  for (let index = 0; index < count; index++) {
    const nonce = Buffer.alloc(12);
    noncePrefix.copy(nonce, 0);
    nonce.writeUInt32BE(index, 8);
    const aad = Buffer.alloc(8);
    aad.write("mt1", 0, "latin1");
    aad.writeUInt32BE(index, 3);
    aad[7] = index === count - 1 ? 1 : 0;
    const cipher = createCipheriv("aes-256-gcm", key, nonce);
    cipher.setAAD(aad);
    const piece = plain.subarray(index * chunkBytes, Math.min(plain.length, (index + 1) * chunkBytes));
    parts.push(cipher.update(piece), cipher.final(), cipher.getAuthTag());
  }
  return Buffer.concat(parts);
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
  readonly nameSecret = randomBytes(32);
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
    const hash = createHash("sha256").update(file.bytes).digest("hex");
    let kept = this.keys.get(hash);
    if (!kept) {
      kept = { key: randomBytes(32), noncePrefix: randomBytes(8) };
      this.keys.set(hash, kept);
    }
    const keyId = createHash("sha256").update("monoptah-transfer-key-id").update(kept.key).digest("hex").slice(0, 16);
    const object = `${createHmac("sha256", this.nameSecret).update(hash).digest("hex")}.${keyId}`;
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
      key: kept.key.toString("base64"),
      nonce: kept.noncePrefix.toString("base64"),
      chunkBytes: TRANSFER_CHUNK_BYTES,
      reused,
      ...(file.assets ? { assets: file.assets } : {}),
      ...(file.project ? { project: file.project } : {}),
    };
  }
}
