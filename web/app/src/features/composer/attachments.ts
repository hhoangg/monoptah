// Copied from src/features/sessions/model/attachments.ts at ca73ac2b
// `isVisionImage`, `normalizeImageMime` and `filesFromClipboard` are copied. Everything that
// touched the Tauri filesystem is gone: files come from `<input type="file">`, paste or drop and
// are read with `FileReader`. The wire shape and the limits follow F-48 and
// mobile/src/features/sessions/attachments.ts.
import { MAX_ATTACHMENTS as PROTOCOL_MAX_ATTACHMENTS } from "@monoptah/protocol";
import type { Attachment as WireAttachment } from "@monoptah/protocol";

/** At most 10 files per message (the desktop's own cap). */
export const MAX_ATTACHMENTS = PROTOCOL_MAX_ATTACHMENTS;
/** 8 MiB of base64 per message: the relay's reassembled request limit (architecture.md 5.2). */
export const MAX_TOTAL_BASE64_BYTES = 8 * 1024 * 1024;
/** Per file, raw bytes: the largest file whose base64 still fits the whole message. */
export const MAX_FILE_BYTES = Math.floor((MAX_TOTAL_BASE64_BYTES * 3) / 4);
/** The size copy shown to the user (F-48). */
export const ATTACHMENT_LIMIT_COPY = "Attachments are limited to 8 MiB per message.";
/** A photo's longer side; larger ones are scaled down, as vision models do anyway. */
export const MAX_IMAGE_SIDE = 2048;
/** JPEG quality of a re-encoded photo. */
export const IMAGE_JPEG_QUALITY = 0.85;
/** Sent as picked when already small and in a format every provider reads. */
const SENT_AS_IS = new Set(["image/png", "image/jpeg", "image/gif", "image/webp"]);
const MAX_AS_IS_BYTES = 1.5 * 1024 * 1024;

/** A file or photo held in the composer until it is sent. */
export type ComposerAttachment = {
  id: string;
  name: string;
  mimeType: string;
  size: number;
  /** A `blob:` URL for the composer thumbnail; revoke it with `revokeAttachment`. */
  previewUrl?: string;
  /** The file's bytes as standard base64, as the desktop takes them. */
  data: string;
};

export class AttachmentError extends Error {
  override name = "AttachmentError";
}

/** MIME types providers typically send as vision input. */
const VISION_MIME = new Set([
  "image/png",
  "image/jpeg",
  "image/jpg",
  "image/gif",
  "image/webp",
]);

const MIME_BY_EXT: Record<string, string> = {
  png: "image/png",
  jpg: "image/jpeg",
  jpeg: "image/jpeg",
  gif: "image/gif",
  webp: "image/webp",
  bmp: "image/bmp",
  ico: "image/x-icon",
  svg: "image/svg+xml",
  tif: "image/tiff",
  tiff: "image/tiff",
  heic: "image/heic",
  heif: "image/heif",
  avif: "image/avif",
  mp3: "audio/mpeg",
  wav: "audio/wav",
  m4a: "audio/mp4",
  aac: "audio/aac",
  ogg: "audio/ogg",
  flac: "audio/flac",
  mp4: "video/mp4",
  mov: "video/quicktime",
  webm: "video/webm",
  mkv: "video/x-matroska",
  pdf: "application/pdf",
  txt: "text/plain",
  md: "text/markdown",
  markdown: "text/markdown",
  csv: "text/csv",
  tsv: "text/tab-separated-values",
  html: "text/html",
  htm: "text/html",
  xml: "application/xml",
  json: "application/json",
  yaml: "text/yaml",
  yml: "text/yaml",
  toml: "application/toml",
  rtf: "application/rtf",
  doc: "application/msword",
  docx: "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
  xls: "application/vnd.ms-excel",
  xlsx: "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet",
  ppt: "application/vnd.ms-powerpoint",
  pptx: "application/vnd.openxmlformats-officedocument.presentationml.presentation",
  zip: "application/zip",
  gz: "application/gzip",
  tar: "application/x-tar",
  ts: "text/plain",
  tsx: "text/plain",
  js: "text/javascript",
  jsx: "text/plain",
  mjs: "text/javascript",
  cjs: "text/javascript",
  css: "text/css",
  rs: "text/plain",
  py: "text/x-python",
  go: "text/plain",
  java: "text/plain",
  kt: "text/plain",
  swift: "text/plain",
  c: "text/plain",
  h: "text/plain",
  cc: "text/plain",
  cpp: "text/plain",
  hpp: "text/plain",
  cs: "text/plain",
  rb: "text/plain",
  php: "text/plain",
  sh: "text/plain",
  zsh: "text/plain",
  bash: "text/plain",
  sql: "application/sql",
  graphql: "application/graphql",
};

export const isImage = (attachment: Pick<ComposerAttachment, "mimeType">) =>
  attachment.mimeType.startsWith("image/");

/** What `conversation.send` and `session.start` take for each attachment. */
export const toWire = (attachments: readonly ComposerAttachment[]): WireAttachment[] =>
  attachments.map(({ name, mimeType, data }) => ({ name, mimeType, data }));

type ClipboardFileItem = {
  kind: string;
  type: string;
  getAsFile: () => File | null;
};

/** Clipboard files plus items. WebKit's FileList is often truncated to the first file. */
export function filesFromClipboard(
  data:
    | {
        files?: ArrayLike<File> | null;
        items?: ArrayLike<ClipboardFileItem> | null;
      }
    | null
    | undefined,
): File[] {
  const fromList = arrayLike(data?.files);
  const fromItems: File[] = [];
  for (const item of arrayLike(data?.items)) {
    if (item.kind !== "file") continue;
    const file = item.getAsFile();
    if (file) fromItems.push(file);
  }
  const files = fromItems.length > fromList.length ? fromItems : fromList;
  return dropMacScreenshotTwins(files);
}

function arrayLike<T>(list: ArrayLike<T> | null | undefined): T[] {
  return list ? Array.from(list) : [];
}

/** macOS often exposes the same screenshot as PNG and an unnamed TIFF. */
function dropMacScreenshotTwins(files: File[]): File[] {
  const unnamedTiff = (file: File) => {
    const type = file.type.toLowerCase();
    if (type !== "image/tiff" && type !== "image/tif") return false;
    const name = file.name.trim().toLowerCase();
    return (
      !name || name === "image.tiff" || name === "image.tif" || name === "image"
    );
  };
  const hasOtherImage = files.some(
    (file) => file.type.startsWith("image/") && !unnamedTiff(file),
  );
  if (!hasOtherImage) return files;
  return files.filter((file) => !unnamedTiff(file));
}

export function isVisionImage(mimeType: string): boolean {
  return VISION_MIME.has(mimeType.toLowerCase());
}

export function normalizeImageMime(mimeType: string): string {
  const mime = mimeType.toLowerCase();
  if (mime === "image/jpg") return "image/jpeg";
  return mime;
}


export function mimeFor(name: string, type: string | null | undefined): string {
  if (type && type !== "application/octet-stream") return type;
  const ext = name.split(".").pop()?.toLowerCase() ?? "";
  return MIME_BY_EXT[ext] ?? "application/octet-stream";
}

/** Total base64 length of the attachments, the figure the 8 MiB cap is measured on. */
export function totalBase64(attachments: readonly Pick<ComposerAttachment, "data">[]): number {
  return attachments.reduce((sum, attachment) => sum + attachment.data.length, 0);
}

/**
 * Adds picks to what is held, keeping at most 10 and refusing any set that would take the message
 * over the link's limit. Throws an `AttachmentError` with user-facing text.
 */
export function mergeAttachments(
  existing: readonly ComposerAttachment[],
  added: readonly ComposerAttachment[],
): ComposerAttachment[] {
  const next = [...existing, ...added].slice(0, MAX_ATTACHMENTS);
  if (totalBase64(next) > MAX_TOTAL_BASE64_BYTES) {
    throw new AttachmentError(
      `These attachments are too large to send together. ${ATTACHMENT_LIMIT_COPY} Remove some and try again.`,
    );
  }
  return next;
}

/** How many more files fit. */
export function attachmentRoom(existing: readonly unknown[]): number {
  return Math.max(0, MAX_ATTACHMENTS - existing.length);
}

/** True when a photo has to be scaled down or re-encoded before it is sent. */
export function imageNeedsResize(image: { mimeType: string; size: number; width: number; height: number }): boolean {
  const small =
    SENT_AS_IS.has(normalizeImageMime(image.mimeType)) &&
    image.size <= MAX_AS_IS_BYTES &&
    Math.max(image.width, image.height) <= MAX_IMAGE_SIDE;
  return !small;
}

/** The size a photo is scaled to: the longer side at most `MAX_IMAGE_SIDE`, ratio kept. */
export function scaledSize(width: number, height: number): { width: number; height: number } {
  const longest = Math.max(width, height);
  if (longest <= MAX_IMAGE_SIDE) return { width, height };
  const scale = MAX_IMAGE_SIDE / longest;
  return { width: Math.max(1, Math.round(width * scale)), height: Math.max(1, Math.round(height * scale)) };
}

/** Standard base64 of a blob's bytes, without the data URL prefix. */
export function readBlobBase64(blob: Blob): Promise<string> {
  return new Promise((resolve, reject) => {
    const reader = new FileReader();
    reader.onload = () => {
      const result = reader.result;
      if (typeof result !== "string") {
        reject(new AttachmentError("Could not read this file."));
        return;
      }
      const comma = result.indexOf(",");
      resolve(comma >= 0 ? result.slice(comma + 1) : result);
    };
    reader.onerror = () => reject(new AttachmentError("Could not read this file."));
    reader.readAsDataURL(blob);
  });
}

const makeId = () => `${Date.now().toString(36)}-${Math.random().toString(36).slice(2, 8)}`;

/** Scales a photo down and re-encodes it as JPEG 0.85 on a canvas, when the browser can decode it. */
async function resizeImage(file: File): Promise<{ blob: Blob; name: string } | null> {
  if (typeof createImageBitmap !== "function") return null;
  let bitmap: ImageBitmap;
  try {
    bitmap = await createImageBitmap(file);
  } catch {
    return null;
  }
  try {
    if (!imageNeedsResize({ mimeType: file.type, size: file.size, width: bitmap.width, height: bitmap.height })) return null;
    const { width, height } = scaledSize(bitmap.width, bitmap.height);
    const canvas = document.createElement("canvas");
    canvas.width = width;
    canvas.height = height;
    const context = canvas.getContext("2d");
    if (!context) return null;
    context.drawImage(bitmap, 0, 0, width, height);
    const blob = await new Promise<Blob | null>((resolve) =>
      canvas.toBlob(resolve, "image/jpeg", IMAGE_JPEG_QUALITY),
    );
    if (!blob) return null;
    const base = (file.name || "photo").replace(/\.[^.]+$/, "");
    return { blob, name: `${base}.jpg` };
  } finally {
    bitmap.close();
  }
}

/**
 * One picked, pasted or dropped file as a composer attachment. A photo is scaled down when it
 * needs it; any other file over the per-file cap is refused.
 */
export async function attachmentFromFile(file: File): Promise<ComposerAttachment> {
  const mimeType = mimeFor(file.name, file.type);
  let blob: Blob = file;
  let name = file.name || fallbackName(mimeType);
  let outType = mimeType;
  if (mimeType.startsWith("image/")) {
    const resized = await resizeImage(file);
    if (resized) {
      blob = resized.blob;
      name = resized.name;
      outType = "image/jpeg";
    }
  }
  if (blob.size > MAX_FILE_BYTES) {
    throw new AttachmentError(`${name} is too large. ${ATTACHMENT_LIMIT_COPY}`);
  }
  const data = await readBlobBase64(blob);
  return {
    id: makeId(),
    name,
    mimeType: outType,
    size: blob.size,
    previewUrl: outType.startsWith("image/") ? URL.createObjectURL(blob) : undefined,
    data,
  };
}

/** Releases a thumbnail's `blob:` URL once the attachment is sent or removed. */
export function revokeAttachment(attachment: Pick<ComposerAttachment, "previewUrl">): void {
  if (attachment.previewUrl) URL.revokeObjectURL(attachment.previewUrl);
}

function fallbackName(mimeType: string): string {
  if (mimeType === "image/png") return "image.png";
  if (mimeType === "image/jpeg" || mimeType === "image/jpg") return "image.jpg";
  if (mimeType === "image/gif") return "image.gif";
  if (mimeType === "image/webp") return "image.webp";
  if (mimeType.startsWith("image/")) return "image";
  if (mimeType.startsWith("audio/")) return "audio";
  return "attachment";
}
