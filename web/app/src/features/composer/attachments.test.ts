// Copied from src/features/sessions/model/attachments.test.ts at ca73ac2b
import { describe, expect, it, vi } from "vitest";
import {
  ATTACHMENT_LIMIT_COPY,
  AttachmentError,
  MAX_ATTACHMENTS,
  MAX_FILE_BYTES,
  MAX_IMAGE_SIDE,
  MAX_TOTAL_BASE64_BYTES,
  attachmentFromFile,
  attachmentRoom,
  filesFromClipboard,
  imageNeedsResize,
  isImage,
  isVisionImage,
  mergeAttachments,
  mimeFor,
  normalizeImageMime,
  revokeAttachment,
  scaledSize,
  toWire,
  totalBase64,
  type ComposerAttachment,
} from "./attachments";

function file(name: string, type: string, body = "x") {
  return new File([body], name, { type });
}

function item(next: File): { kind: string; type: string; getAsFile: () => File | null } {
  return { kind: "file", type: next.type, getAsFile: () => next };
}

function attachment(partial: Partial<ComposerAttachment> & Pick<ComposerAttachment, "id" | "name">): ComposerAttachment {
  return { mimeType: "image/png", size: 4, data: "AAAA", ...partial };
}

describe("limits", () => {
  it("are the web limits: 10 files and 8 MiB of base64 per message", () => {
    expect(MAX_ATTACHMENTS).toBe(10);
    expect(MAX_TOTAL_BASE64_BYTES).toBe(8 * 1024 * 1024);
    expect(Math.ceil((MAX_FILE_BYTES * 4) / 3)).toBeLessThanOrEqual(MAX_TOTAL_BASE64_BYTES);
    expect(ATTACHMENT_LIMIT_COPY).toBe("Attachments are limited to 8 MiB per message.");
  });
});

describe("mergeAttachments", () => {
  it("keeps previously attached files when adding more", () => {
    const merged = mergeAttachments([attachment({ id: "1", name: "a.png" })], [attachment({ id: "2", name: "b.png" })]);
    expect(merged.map((a) => a.id)).toEqual(["1", "2"]);
  });

  it("keeps at most 10 files", () => {
    const many = Array.from({ length: 12 }, (_, i) => attachment({ id: String(i), name: `${i}.png` }));
    expect(mergeAttachments([], many)).toHaveLength(MAX_ATTACHMENTS);
    expect(attachmentRoom(many.slice(0, 4))).toBe(6);
    expect(attachmentRoom(many)).toBe(0);
  });

  it("refuses a set over 8 MiB of base64", () => {
    const big = attachment({ id: "big", name: "big.bin", data: "A".repeat(MAX_TOTAL_BASE64_BYTES) });
    expect(mergeAttachments([], [big])).toHaveLength(1);
    const more = attachment({ id: "more", name: "more.bin", data: "A" });
    expect(() => mergeAttachments([big], [more])).toThrow(AttachmentError);
    expect(totalBase64([big, more])).toBe(MAX_TOTAL_BASE64_BYTES + 1);
  });
});

describe("filesFromClipboard", () => {
  it("returns every file item when the files list is truncated", () => {
    const a = file("a.png", "image/png", "a");
    const b = file("b.png", "image/png", "b");
    expect(filesFromClipboard({ files: [a], items: [item(a), item(b)] })).toEqual([a, b]);
  });

  it("drops the unnamed tiff twin of a png screenshot", () => {
    const png = file("image.png", "image/png");
    const tiff = file("image.tiff", "image/tiff");
    expect(filesFromClipboard({ files: [png], items: [item(png), item(tiff)] })).toEqual([png]);
  });

  it("keeps a real named tiff next to a png", () => {
    const png = file("diagram.png", "image/png");
    const tiff = file("scan.tiff", "image/tiff");
    expect(filesFromClipboard({ files: [png, tiff], items: [item(png), item(tiff)] })).toEqual([png, tiff]);
  });

  it("handles missing clipboard data and non-file items", () => {
    expect(filesFromClipboard(null)).toEqual([]);
    expect(filesFromClipboard({ items: [{ kind: "string", type: "text/plain", getAsFile: () => null }] })).toEqual([]);
  });
});

describe("mime helpers", () => {
  it("knows which images providers take as vision input", () => {
    expect(isVisionImage("image/png")).toBe(true);
    expect(isVisionImage("IMAGE/JPG")).toBe(true);
    expect(isVisionImage("image/svg+xml")).toBe(false);
    expect(isVisionImage("application/pdf")).toBe(false);
  });

  it("normalizes image/jpg and case", () => {
    expect(normalizeImageMime("image/jpg")).toBe("image/jpeg");
    expect(normalizeImageMime("IMAGE/PNG")).toBe("image/png");
  });

  it("picks a mime from the type, then the extension", () => {
    expect(mimeFor("a.bin", "text/plain")).toBe("text/plain");
    expect(mimeFor("notes.md", "")).toBe("text/markdown");
    expect(mimeFor("x.unknown", "application/octet-stream")).toBe("application/octet-stream");
    expect(isImage({ mimeType: "image/webp" })).toBe(true);
    expect(isImage({ mimeType: "text/plain" })).toBe(false);
  });
});

describe("wire shape", () => {
  it("sends only name, mimeType and data", () => {
    expect(toWire([attachment({ id: "1", name: "a.png", previewUrl: "blob:x" })])).toEqual([
      { name: "a.png", mimeType: "image/png", data: "AAAA" },
    ]);
  });
});

describe("photo scaling", () => {
  it("leaves a small supported photo as picked and scales the rest", () => {
    expect(imageNeedsResize({ mimeType: "image/png", size: 1000, width: 800, height: 600 })).toBe(false);
    expect(imageNeedsResize({ mimeType: "image/png", size: 2_000_000, width: 800, height: 600 })).toBe(true);
    expect(imageNeedsResize({ mimeType: "image/jpeg", size: 1000, width: 4000, height: 300 })).toBe(true);
    expect(imageNeedsResize({ mimeType: "image/heic", size: 1000, width: 100, height: 100 })).toBe(true);
  });

  it("scales the longer side to 2048 and keeps the ratio", () => {
    expect(scaledSize(4096, 2048)).toEqual({ width: MAX_IMAGE_SIDE, height: 1024 });
    expect(scaledSize(1000, 3000)).toEqual({ width: 683, height: MAX_IMAGE_SIDE });
    expect(scaledSize(100, 50)).toEqual({ width: 100, height: 50 });
  });
});

describe("attachmentFromFile", () => {
  it("reads a text file as base64 with the right mime and no preview", async () => {
    const result = await attachmentFromFile(new File(["hello"], "notes.md", { type: "" }));
    expect(result).toMatchObject({ name: "notes.md", mimeType: "text/markdown", size: 5, data: "aGVsbG8=" });
    expect(result.previewUrl).toBeUndefined();
  });

  it("gives a nameless pasted image a name and a blob preview", async () => {
    const create = vi.fn().mockReturnValue("blob:preview");
    const revoke = vi.fn();
    vi.stubGlobal("URL", Object.assign(URL, { createObjectURL: create, revokeObjectURL: revoke }));
    try {
      const result = await attachmentFromFile(new File(["png"], "", { type: "image/png" }));
      expect(result.name).toBe("image.png");
      expect(result.previewUrl).toBe("blob:preview");
      revokeAttachment(result);
      expect(revoke).toHaveBeenCalledWith("blob:preview");
    } finally {
      vi.unstubAllGlobals();
    }
  });

  it("refuses a file over the per-file cap", async () => {
    const huge = new File([new Uint8Array(MAX_FILE_BYTES + 1)], "huge.bin", { type: "application/octet-stream" });
    await expect(attachmentFromFile(huge)).rejects.toThrow(/8 MiB per message/);
  });
});
