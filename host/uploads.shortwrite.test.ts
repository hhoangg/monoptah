import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { createHash } from "node:crypto";
import { mkdtempSync, readFileSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";

const mode = vi.hoisted(() => ({ short: false }));
vi.mock("node:fs", async (importOriginal) => {
  const actual = await importOriginal<typeof import("node:fs")>();
  return {
    ...actual,
    writeSync: ((fd: number, buffer: Buffer, offset: number, length: number, position: number) =>
      mode.short
        ? actual.writeSync(fd, buffer, offset, Math.max(0, length - 1), position)
        : actual.writeSync(fd, buffer, offset, length, position)) as typeof actual.writeSync,
  };
});
import { HostUploads } from "./uploads";

let directory: string;
beforeEach(() => {
  directory = mkdtempSync(join(tmpdir(), "host-uploads-short-"));
});
afterEach(() => rmSync(directory, { recursive: true, force: true }));

it("rolls back a short write and does not advance the offset", () => {
  const uploads = new HostUploads(join(directory, "uploads"));
  const data = Buffer.from("abcdefgh");
  const sha256 = createHash("sha256").update(data).digest("hex");
  const { uploadId } = uploads.begin({ name: "a", size: 8, sha256 });
  uploads.chunk({ uploadId, offset: 0, data: data.subarray(0, 4).toString("base64") });
  mode.short = true;
  expect(() =>
    uploads.chunk({ uploadId, offset: 4, data: data.subarray(4).toString("base64") }),
  ).toThrow("not fully written");
  mode.short = false;
  expect(readFileSync(join(directory, "uploads", uploadId, "part")).length).toBe(4);
  expect(uploads.begin({ name: "a", size: 8, sha256, uploadId }).offset).toBe(4);
  uploads.chunk({ uploadId, offset: 4, data: data.subarray(4).toString("base64") });
  expect(readFileSync(uploads.commit({ uploadId }).path)).toEqual(data);
});
