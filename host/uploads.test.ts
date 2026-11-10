import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { createHash } from "node:crypto";
import {
  existsSync,
  mkdirSync,
  mkdtempSync,
  readFileSync,
  readdirSync,
  rmSync,
  statSync,
  utimesSync,
} from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import {
  HostUploads,
  MAX_LIVE_UPLOADS,
  MAX_UPLOAD_BYTES,
  MAX_UPLOAD_QUOTA_BYTES,
  UPLOAD_CHUNK_BYTES,
  UPLOAD_SWEEP_INTERVAL_MS,
  UPLOAD_TTL_MS,
} from "./uploads";

let directory: string;
let uploads: HostUploads;
beforeEach(() => {
  directory = mkdtempSync(join(tmpdir(), "host-uploads-test-"));
  uploads = new HostUploads(join(directory, "uploads"));
});
afterEach(() => rmSync(directory, { recursive: true, force: true }));

const sha = (data: Buffer | string) => createHash("sha256").update(data).digest("hex");
const b64 = (data: Buffer | string) => Buffer.from(data).toString("base64");
const begin = (data: Buffer, name = "kit.tar", extra: Record<string, unknown> = {}) =>
  uploads.begin({ name, size: data.length, sha256: sha(data), ...extra });
const send = (uploadId: string, offset: number, data: Buffer | string) =>
  uploads.chunk({ uploadId, offset, data: b64(data) });

it("stages in 512 KB chunks and commits a 0600 regular file in a 0700 dir", () => {
  const data = Buffer.alloc(UPLOAD_CHUNK_BYTES * 2 + 100, 7);
  const { uploadId, offset, chunkSize } = begin(data);
  expect([offset, chunkSize]).toEqual([0, UPLOAD_CHUNK_BYTES]);
  expect(uploadId).toMatch(/^[0-9a-f]{32}$/);
  for (let at = 0; at < data.length; at += chunkSize)
    expect(send(uploadId, at, data.subarray(at, at + chunkSize))).toEqual({
      offset: Math.min(at + chunkSize, data.length),
    });
  const done = uploads.commit({ uploadId });
  expect(done).toMatchObject({ name: "kit.tar", size: data.length, sha256: sha(data) });
  expect(done.path).toBe(join(directory, "uploads", uploadId, "files", "kit.tar"));
  expect(readFileSync(done.path).equals(data)).toBe(true);
  if (process.platform !== "win32") {
    expect(statSync(done.path).mode & 0o777).toBe(0o600);
    expect(statSync(join(directory, "uploads", uploadId)).mode & 0o777).toBe(0o700);
    expect(statSync(join(directory, "uploads")).mode & 0o777).toBe(0o700);
  }
  expect(statSync(done.path).isFile()).toBe(true);
  expect(uploads.commit({ uploadId })).toEqual(done);
  expect(uploads.committedPath(uploadId)).toBe(done.path);
  expect(() => send(uploadId, 0, "x")).toThrow("already committed");
});

it("accepts idempotent retries and rejects gaps and conflicting overlaps", () => {
  const data = Buffer.from("abcdefgh");
  const { uploadId } = begin(data);
  expect(send(uploadId, 0, "abcd")).toEqual({ offset: 4 });
  expect(send(uploadId, 0, "abcd")).toEqual({ offset: 4 });
  expect(() => send(uploadId, 0, "xbcd")).toThrow("does not match");
  expect(() => send(uploadId, 2, "cxef")).toThrow("does not match");
  expect(() => send(uploadId, 5, "fgh")).toThrow("out of order");
  expect(send(uploadId, 2, "cdef")).toEqual({ offset: 6 });
  expect(send(uploadId, 6, "gh")).toEqual({ offset: 8 });
  expect(uploads.commit({ uploadId }).size).toBe(8);
});

it("resumes from the offset the Host holds", () => {
  const data = Buffer.from("0123456789");
  const first = begin(data);
  send(first.uploadId, 0, "01234");
  const resumed = begin(data, "kit.tar", { uploadId: first.uploadId });
  expect(resumed).toEqual({ uploadId: first.uploadId, offset: 5, chunkSize: UPLOAD_CHUNK_BYTES });
  send(first.uploadId, 5, "56789");
  expect(uploads.commit({ uploadId: first.uploadId }).sha256).toBe(sha(data));
  expect(() => begin(Buffer.from("other"), "kit.tar", { uploadId: first.uploadId })).toThrow(
    "does not match",
  );
  expect(() => begin(data, "kit.tar", { uploadId: "0".repeat(32) })).toThrow("not found");
});

it("fails a hash mismatch and drops the bad bytes", () => {
  const { uploadId } = uploads.begin({ name: "a.bin", size: 3, sha256: sha("abc") });
  send(uploadId, 0, "abd");
  expect(() => uploads.commit({ uploadId })).toThrow("sha256 does not match");
  expect(existsSync(join(directory, "uploads", uploadId))).toBe(false);
});

it("refuses to commit an incomplete upload", () => {
  const { uploadId } = begin(Buffer.from("abcd"));
  send(uploadId, 0, "ab");
  expect(() => uploads.commit({ uploadId })).toThrow("incomplete");
});

it("enforces the size cap at begin and as bytes arrive", () => {
  expect(() =>
    uploads.begin({ name: "big", size: MAX_UPLOAD_BYTES + 1, sha256: sha("") }),
  ).toThrow("larger than 500 MB");
  expect(() => uploads.begin({ name: "big", size: -1, sha256: sha("") })).toThrow("size");
  expect(() => uploads.begin({ name: "big", size: 1.5, sha256: sha("") })).toThrow("size");
  const { uploadId } = begin(Buffer.from("abc"));
  expect(() => send(uploadId, 0, "abcd")).toThrow("exceeds the declared size");
  expect(() => send(uploadId, 3, "d")).toThrow("exceeds the declared size");
  expect(() => send(uploadId, 0, Buffer.alloc(UPLOAD_CHUNK_BYTES + 1))).toThrow();
  expect(statSync(join(directory, "uploads", uploadId, "part")).size).toBe(0);
});

it("expires uploads idle for an hour, at begin and at startup", () => {
  const old = begin(Buffer.from("x"));
  const kept = begin(Buffer.from("y"));
  const past = new Date(Date.now() - UPLOAD_TTL_MS - 60_000);
  utimesSync(join(directory, "uploads", old.uploadId, "meta.json"), past, past);
  begin(Buffer.from("z"));
  expect(existsSync(join(directory, "uploads", old.uploadId))).toBe(false);
  expect(existsSync(join(directory, "uploads", kept.uploadId))).toBe(true);
  expect(() => send(old.uploadId, 0, "x")).toThrow("not found");

  utimesSync(join(directory, "uploads", kept.uploadId, "meta.json"), past, past);
  new HostUploads(join(directory, "uploads"));
  expect(existsSync(join(directory, "uploads", kept.uploadId))).toBe(false);
});

it("expires by the injected clock and activity keeps an upload alive", () => {
  let now = Date.now();
  const clocked = new HostUploads(join(directory, "clocked"), () => now);
  const data = Buffer.from("abcd");
  const { uploadId } = clocked.begin({ name: "a", size: 4, sha256: sha(data) });
  now += UPLOAD_TTL_MS - 1000;
  clocked.chunk({ uploadId, offset: 0, data: b64("ab") });
  now += UPLOAD_TTL_MS - 1000;
  expect(clocked.sweep()).toBe(0);
  now += 2000;
  expect(clocked.sweep()).toBe(1);
});

it.each([
  "",
  ".",
  "..",
  "a/b",
  "../x",
  "a\\b",
  "a\0b",
  ".hidden",
  ".ssh",
  "trail.",
  " pad",
  "pad ",
  "new\nline",
  "x".repeat(256),
  "part",
  "PART",
  "meta.json",
  "META.json",
  "Meta.json",
  "Files",
  "ＭＥＴＡ.json",
  "name.",
  "name ",
  "CON",
  "a:b",
  "C:\\x",
])("rejects the upload name %j", (name) => {
  expect(() => uploads.begin({ name, size: 1, sha256: sha("a") })).toThrow("Invalid upload name");
});

it("rejects non-string names and accepts ordinary ones", () => {
  for (const name of [undefined, 5, null, ["a"], {}])
    expect(() => uploads.begin({ name, size: 1, sha256: sha("a") })).toThrow("Invalid upload name");
  for (const name of ["kit.tar", "my file (1).tar.gz", "résumé.pdf", "a..b"])
    expect(uploads.begin({ name, size: 1, sha256: sha("a") }).uploadId).toBeTruthy();
});

it("rejects a bad hash", () => {
  for (const hash of [undefined, "", "abc", "z".repeat(64)])
    expect(() => uploads.begin({ name: "a", size: 1, sha256: hash })).toThrow("hash");
});

it("rejects traversal and malformed upload ids on every method", () => {
  const outside = join(directory, "outside");
  mkdirSync(outside);
  for (const uploadId of ["..", "../outside", "../../etc", "a/b", "", "0".repeat(31),
    "0".repeat(33), "G".repeat(32), "A".repeat(32), undefined, 5, null, "0".repeat(32) + "/.."]) {
    expect(() => uploads.chunk({ uploadId, offset: 0, data: "" })).toThrow();
    expect(() => uploads.commit({ uploadId })).toThrow();
    expect(() => uploads.discard({ uploadId })).toThrow("Invalid upload ID");
    expect(() => uploads.committedPath(uploadId)).toThrow();
    if (uploadId !== undefined)
      expect(() => begin(Buffer.from("a"), "a", { uploadId })).toThrow();
  }
  expect(existsSync(outside)).toBe(true);
});

it("validates chunk offsets and base64", () => {
  const { uploadId } = begin(Buffer.from("abcd"));
  for (const offset of [-1, 1.5, "0", undefined, Number.MAX_SAFE_INTEGER + 2])
    expect(() => uploads.chunk({ uploadId, offset, data: "" })).toThrow("Invalid upload chunk");
  for (const data of [undefined, 5, "a", "a b=", "!!!!"])
    expect(() => uploads.chunk({ uploadId, offset: 0, data })).toThrow("Invalid upload chunk");
});

it("discards an upload, committed or not, and tolerates a repeat", () => {
  const a = begin(Buffer.from("a"));
  expect(uploads.discard({ uploadId: a.uploadId })).toEqual({ discarded: true });
  expect(uploads.discard({ uploadId: a.uploadId })).toEqual({ discarded: false });
  const b = begin(Buffer.from("b"), "b.txt");
  send(b.uploadId, 0, "b");
  const done = uploads.commit({ uploadId: b.uploadId });
  uploads.discard({ uploadId: b.uploadId });
  expect(existsSync(done.path)).toBe(false);
  expect(readdirSync(join(directory, "uploads"))).toEqual([]);
});

it("caps the staging quota and the number of live uploads", () => {
  const big = MAX_UPLOAD_BYTES;
  const count = Math.floor(MAX_UPLOAD_QUOTA_BYTES / big);
  const ids = Array.from({ length: count }, () =>
    uploads.begin({ name: "big", size: big, sha256: sha("") }).uploadId,
  );
  expect(() => uploads.begin({ name: "big", size: big, sha256: sha("") })).toThrow("quota");
  uploads.discard({ uploadId: ids[0] });
  expect(uploads.begin({ name: "big", size: big, sha256: sha("") }).uploadId).toBeTruthy();
  uploads.clear();
  for (let i = 0; i < MAX_LIVE_UPLOADS; i++) begin(Buffer.from("a"));
  expect(() => begin(Buffer.from("a"))).toThrow("Too many uploads");
});

it("sweeps stray entries that are not upload directories", () => {
  begin(Buffer.from("a"));
  const stray = join(directory, "uploads", "strange");
  mkdirSync(stray);
  expect(uploads.sweep()).toBe(1);
  expect(existsSync(stray)).toBe(false);
});

it.each(["META.json", "Meta.json", "PART", "trailing.", "trailing "])(
  "never lets the name %j touch internal files",
  (name) => {
    const data = Buffer.from("payload");
    let id: string | undefined;
    try {
      id = uploads.begin({ name, size: data.length, sha256: sha(data) }).uploadId;
    } catch (error) {
      expect(String(error)).toContain("Invalid upload name");
      return;
    }
    send(id, 0, data);
    expect(readFileSync(uploads.commit({ uploadId: id }).path)).toEqual(data);
  },
);

it("keeps a payload separate from metadata even if the name were hostile", () => {
  const data = Buffer.from("payload");
  const { uploadId } = begin(data, "meta-json");
  send(uploadId, 0, data);
  const { path } = uploads.commit({ uploadId });
  expect(path).toContain(join(uploadId, "files"));
  expect(JSON.parse(readFileSync(join(directory, "uploads", uploadId, "meta.json"), "utf8")).committed).toBe(true);
  expect(readFileSync(path)).toEqual(data);
});

it("sweeps expired uploads on a timer and on chunk, and clears the timer", () => {
  vi.useFakeTimers();
  try {
    let now = Date.now();
    const timed = new HostUploads(join(directory, "timed"), () => now);
    const a = timed.begin({ name: "a", size: 1, sha256: sha("a") });
    timed.startSweeper();
    timed.startSweeper();
    expect(vi.getTimerCount()).toBe(1);
    now += UPLOAD_TTL_MS + 1000;
    vi.advanceTimersByTime(UPLOAD_SWEEP_INTERVAL_MS);
    expect(existsSync(join(directory, "timed", a.uploadId))).toBe(false);
    // chunk and commit also sweep other expired uploads
    const b = timed.begin({ name: "b", size: 1, sha256: sha("b") });
    now += UPLOAD_TTL_MS - 1000;
    const c = timed.begin({ name: "c", size: 1, sha256: sha("c") });
    now += 2000;
    expect(() => timed.chunk({ uploadId: c.uploadId, offset: 0, data: b64("c") })).not.toThrow();
    expect(existsSync(join(directory, "timed", b.uploadId))).toBe(false);
    timed.stopSweeper();
    expect(vi.getTimerCount()).toBe(0);
  } finally {
    vi.useRealTimers();
  }
});
