import { createHash, randomBytes } from "node:crypto";
import {
  chmodSync,
  closeSync,
  existsSync,
  fstatSync,
  ftruncateSync,
  mkdirSync,
  openSync,
  readFileSync,
  readSync,
  readdirSync,
  renameSync,
  rmSync,
  statSync,
  utimesSync,
  writeFileSync,
  writeSync,
} from "node:fs";
import { join } from "node:path";

/** Generic staged uploads: chunked, resumable and verified by sha256. */
export const UPLOAD_CHUNK_BYTES = 512 * 1024;
export const MAX_UPLOAD_BYTES = 500 * 1024 * 1024;
export const UPLOAD_TTL_MS = 60 * 60 * 1000;
/** Declared bytes across every live upload, so one client cannot fill the disk. */
export const MAX_UPLOAD_QUOTA_BYTES = 2 * 1024 * 1024 * 1024;
export const MAX_LIVE_UPLOADS = 32;
const MAX_NAME_BYTES = 255;
const ID = /^[0-9a-f]{32}$/;
const SHA256 = /^[0-9a-f]{64}$/;
const PART = "part";
const META = "meta.json";
/** Committed payloads live in this subdirectory, so no user-supplied name can
 * collide with the internal files, even on a case-insensitive filesystem. */
const FILES = "files";
export const UPLOAD_SWEEP_INTERVAL_MS = 5 * 60 * 1000;
const RESERVED = new Set([PART, META, FILES]);
const WINDOWS_RESERVED = /^(con|prn|aux|nul|com[1-9]|lpt[1-9])(\..*)?$/i;

interface Meta {
  name: string;
  size: number;
  sha256: string;
  committed: boolean;
}

export interface UploadStatus {
  uploadId: string;
  offset: number;
  chunkSize: number;
}

/** One plain file name: no separators, no traversal, no control characters. */
export function validateUploadName(value: unknown): string {
  if (typeof value !== "string") throw new Error("Invalid upload name");
  if (
    !value ||
    Buffer.byteLength(value) > MAX_NAME_BYTES ||
    value !== value.trim() ||
    value.startsWith(".") ||
    value.endsWith(".") ||
    RESERVED.has(value.normalize("NFKC").toLowerCase()) ||
    // eslint-disable-next-line no-control-regex
    /[\u0000-\u001f\u007f/\\:*?"<>|]/.test(value) ||
    WINDOWS_RESERVED.test(value)
  )
    throw new Error("Invalid upload name");
  return value;
}

export function validateUploadId(value: unknown): string {
  if (typeof value !== "string" || !ID.test(value))
    throw new Error("Invalid upload ID");
  return value;
}

function sizeOf(value: unknown): number {
  if (!Number.isSafeInteger(value) || Number(value) < 0)
    throw new Error("Invalid upload size");
  if (Number(value) > MAX_UPLOAD_BYTES)
    throw new Error("Upload is larger than 500 MB");
  return Number(value);
}

export class HostUploads {
  constructor(
    readonly root: string,
    private readonly now: () => number = Date.now,
  ) {
    this.sweep();
  }

  private timer?: ReturnType<typeof setInterval>;

  /** Sweeps expired uploads on a timer that never keeps the process alive. */
  startSweeper(intervalMs: number = UPLOAD_SWEEP_INTERVAL_MS): void {
    if (this.timer) return;
    this.timer = setInterval(() => this.sweep(), intervalMs);
    this.timer.unref();
  }

  stopSweeper(): void {
    if (this.timer) clearInterval(this.timer);
    this.timer = undefined;
  }

  private payload(id: string, meta: Meta): string {
    return join(this.dir(id), FILES, meta.name);
  }

  private dir(id: string): string {
    return join(this.root, validateUploadId(id));
  }

  private ensureRoot(): void {
    mkdirSync(this.root, { recursive: true, mode: 0o700 });
    chmodSync(this.root, 0o700);
  }

  private meta(id: string): Meta {
    try {
      return JSON.parse(readFileSync(join(this.dir(id), META), "utf8")) as Meta;
    } catch {
      throw new Error("Upload not found");
    }
  }

  private touch(id: string): void {
    const at = new Date(this.now());
    utimesSync(join(this.dir(id), META), at, at);
  }

  /** Removes uploads idle for longer than the TTL, and unreadable leftovers. */
  sweep(): number {
    let removed = 0;
    let entries: string[];
    try {
      entries = readdirSync(this.root);
    } catch {
      return 0;
    }
    for (const entry of entries) {
      const path = join(this.root, entry);
      let idle = Infinity;
      try {
        idle = this.now() - statSync(join(path, META)).mtimeMs;
      } catch {
        try {
          idle = this.now() - statSync(path).mtimeMs;
        } catch {
          continue;
        }
      }
      if (!ID.test(entry) || idle > UPLOAD_TTL_MS) {
        rmSync(path, { recursive: true, force: true });
        removed++;
      }
    }
    return removed;
  }

  /** Starts an upload, or resumes the one named by `uploadId` (same name,
   * size and hash) and reports how many bytes the Host already holds. */
  begin(input: Record<string, unknown>): UploadStatus {
    const name = validateUploadName(input.name);
    const size = sizeOf(input.size);
    const sha256 = String(input.sha256 ?? "").toLowerCase();
    if (!SHA256.test(sha256)) throw new Error("Invalid upload hash");
    this.sweep();
    if (input.uploadId !== undefined) {
      const id = validateUploadId(input.uploadId);
      const meta = this.meta(id);
      if (meta.name !== name || meta.size !== size || meta.sha256 !== sha256)
        throw new Error("Upload does not match the one being resumed");
      this.touch(id);
      return { uploadId: id, offset: this.offset(id, meta), chunkSize: UPLOAD_CHUNK_BYTES };
    }
    this.ensureRoot();
    const live = readdirSync(this.root).filter((entry) => ID.test(entry));
    if (live.length >= MAX_LIVE_UPLOADS) throw new Error("Too many uploads in progress");
    let reserved = 0;
    for (const entry of live) {
      try {
        reserved += this.meta(entry).size;
      } catch {
        // A directory without readable metadata holds nothing to count.
      }
    }
    if (reserved + size > MAX_UPLOAD_QUOTA_BYTES)
      throw new Error("Upload staging quota exceeded");
    const uploadId = randomBytes(16).toString("hex");
    const dir = this.dir(uploadId);
    mkdirSync(dir, { mode: 0o700 });
    chmodSync(dir, 0o700);
    writeFileSync(
      join(dir, META),
      JSON.stringify({ name, size, sha256, committed: false } satisfies Meta),
      { mode: 0o600 },
    );
    writeFileSync(join(dir, PART), "", { mode: 0o600 });
    this.touch(uploadId);
    return { uploadId, offset: 0, chunkSize: UPLOAD_CHUNK_BYTES };
  }

  private offset(id: string, meta: Meta): number {
    return statSync(meta.committed ? this.payload(id, meta) : join(this.dir(id), PART)).size;
  }

  /** A repeated chunk at the same offset is accepted when its bytes match; a
   * gap, or an overlap with different bytes, is an error. */
  chunk(input: Record<string, unknown>): { offset: number } {
    this.sweep();
    const id = validateUploadId(input.uploadId);
    const offset = input.offset;
    const encoded = input.data;
    if (
      !Number.isSafeInteger(offset) ||
      Number(offset) < 0 ||
      typeof encoded !== "string" ||
      encoded.length > Math.ceil(UPLOAD_CHUNK_BYTES / 3) * 4 ||
      (encoded.length > 0 &&
        (!/^[A-Za-z0-9+/]*={0,2}$/.test(encoded) || encoded.length % 4))
    )
      throw new Error("Invalid upload chunk");
    const bytes = Buffer.from(encoded, "base64");
    const meta = this.meta(id);
    if (meta.committed) throw new Error("Upload is already committed");
    const start = Number(offset);
    // The cap is enforced as bytes arrive, against the size declared at begin
    // (which begin already bounded to 500 MB).
    if (bytes.length > UPLOAD_CHUNK_BYTES || start + bytes.length > meta.size)
      throw new Error("Upload chunk exceeds the declared size");
    let fd: number;
    try {
      fd = openSync(join(this.dir(id), PART), "r+");
    } catch {
      throw new Error("Upload not found");
    }
    try {
      const length = fstatSync(fd).size;
      if (start > length) throw new Error("Upload chunks are out of order");
      const overlap = Math.min(bytes.length, length - start);
      if (overlap > 0) {
        const existing = Buffer.alloc(overlap);
        readSync(fd, existing, 0, overlap, start);
        if (!existing.equals(bytes.subarray(0, overlap)))
          throw new Error("Upload retry does not match uploaded bytes");
      }
      if (overlap < bytes.length) {
        const wanted = bytes.length - overlap;
        let written = -1;
        try {
          written = writeSync(fd, bytes, overlap, wanted, start + overlap);
        } finally {
          // A short or failed write must not leave a partial chunk behind.
          if (written !== wanted) ftruncateSync(fd, length);
        }
        if (written !== wanted) throw new Error("Upload chunk was not fully written");
      }
      this.touch(id);
      return { offset: Math.max(length, start + bytes.length) };
    } finally {
      closeSync(fd);
    }
  }

  /** Verifies size and sha256, then publishes `<id>/<name>` as a 0600 file. */
  commit(input: Record<string, unknown>): { uploadId: string; name: string; path: string; size: number; sha256: string } {
    this.sweep();
    const id = validateUploadId(input.uploadId);
    const meta = this.meta(id);
    const dir = this.dir(id);
    const path = this.payload(id, meta);
    const done = () => ({ uploadId: id, name: meta.name, path, size: meta.size, sha256: meta.sha256 });
    if (meta.committed) return done();
    // A crash between the rename and the metadata write leaves the bytes in
    // files/ with committed:false, so a retry finishes from whichever exists.
    const part = join(dir, PART);
    const source = existsSync(part) ? part : existsSync(path) ? path : undefined;
    if (!source) {
      rmSync(dir, { recursive: true, force: true });
      throw new Error("Upload not found");
    }
    if (statSync(source).size !== meta.size) throw new Error("Upload is incomplete");
    const hash = createHash("sha256");
    const fd = openSync(source, "r");
    try {
      const buffer = Buffer.alloc(UPLOAD_CHUNK_BYTES);
      for (let read = readSync(fd, buffer, 0, buffer.length, null); read > 0;
        read = readSync(fd, buffer, 0, buffer.length, null))
        hash.update(buffer.subarray(0, read));
    } finally {
      closeSync(fd);
    }
    if (hash.digest("hex") !== meta.sha256) {
      // The staged bytes are wrong, so a retry must start from scratch.
      rmSync(dir, { recursive: true, force: true });
      throw new Error("Upload sha256 does not match");
    }
    if (source === part) {
      chmodSync(part, 0o600);
      mkdirSync(join(dir, FILES), { recursive: true, mode: 0o700 });
      renameSync(part, path);
    }
    chmodSync(path, 0o600);
    const next = join(dir, `${META}.tmp`);
    writeFileSync(next, JSON.stringify({ ...meta, committed: true }), { mode: 0o600 });
    renameSync(next, join(dir, META));
    this.touch(id);
    return done();
  }

  /** The committed file for a later consumer (for example a kit apply). */
  committedPath(uploadId: unknown): string {
    const id = validateUploadId(uploadId);
    const meta = this.meta(id);
    if (!meta.committed) throw new Error("Upload is not committed");
    return this.payload(id, meta);
  }

  discard(input: Record<string, unknown>): { discarded: boolean } {
    const dir = this.dir(validateUploadId(input.uploadId));
    let existed = true;
    try {
      statSync(dir);
    } catch {
      existed = false;
    }
    rmSync(dir, { recursive: true, force: true });
    return { discarded: existed };
  }

  /** Optional cleanup on Host shutdown. */
  clear(): void {
    this.stopSweeper();
    rmSync(this.root, { recursive: true, force: true });
  }
}
