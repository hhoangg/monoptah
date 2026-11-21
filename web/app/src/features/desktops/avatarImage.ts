// Prepares an uploaded image as a desktop avatar in the browser: only PNG, JPEG and WebP are accepted (decided by the
// first bytes, not by the file name or the type the picker reports), the picture is decoded, cropped to a square and
// re-encoded at 128 px, and the result must fit the server cap. SVG never gets this far, and the Worker checks the
// bytes again.
import { DESKTOP_AVATAR_MAX_BYTES } from "@monoptah/protocol";

export const AVATAR_SIZE = 128;
/** The largest file read from the picker; a bigger one is refused before it is decoded. */
export const AVATAR_SOURCE_MAX_BYTES = 5 * 1024 * 1024;

export type AvatarImageMime = "image/png" | "image/jpeg" | "image/webp";
export type AvatarImageFailure = "unsupported" | "too_large" | "unreadable";

export class AvatarImageError extends Error {
  readonly reason: AvatarImageFailure;
  constructor(reason: AvatarImageFailure) {
    super(reason);
    this.name = "AvatarImageError";
    this.reason = reason;
  }
}

export const AVATAR_IMAGE_MESSAGE: Record<AvatarImageFailure, string> = {
  unsupported: "Use a PNG, JPEG or WebP image.",
  too_large: "That image is too large. Pick one under 5 MB.",
  unreadable: "That image could not be read. Try another one.",
};

const startsWith = (bytes: Uint8Array, prefix: readonly number[], offset = 0): boolean => prefix.every((value, index) => bytes[offset + index] === value);

/** The format of `bytes` by its magic bytes; null for SVG, GIF, HTML and everything else. */
export function sniffImage(bytes: Uint8Array): AvatarImageMime | null {
  if (bytes.length >= 8 && startsWith(bytes, [0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a])) return "image/png";
  if (bytes.length >= 3 && startsWith(bytes, [0xff, 0xd8, 0xff])) return "image/jpeg";
  if (bytes.length >= 12 && startsWith(bytes, [0x52, 0x49, 0x46, 0x46]) && startsWith(bytes, [0x57, 0x45, 0x42, 0x50], 8)) return "image/webp";
  return null;
}

/** The centered square of a `width` x `height` picture: what gets scaled into the avatar. */
export function coverSquare(width: number, height: number): { x: number; y: number; side: number } {
  const side = Math.min(width, height);
  return { x: Math.floor((width - side) / 2), y: Math.floor((height - side) / 2), side };
}

/** Standard base64 of bytes. */
export function bytesToBase64(bytes: Uint8Array): string {
  let binary = "";
  for (let index = 0; index < bytes.length; index += 0x8000) binary += String.fromCharCode(...bytes.subarray(index, index + 0x8000));
  return btoa(binary);
}

function canvasBlob(canvas: HTMLCanvasElement, type: string, quality: number): Promise<Blob | null> {
  return new Promise((resolve) => canvas.toBlob(resolve, type, quality));
}

/**
 * The picked file as `{ mime, data }` for `PATCH /api/desktops/:id`: a 128 px square, WebP when the browser can encode
 * it and JPEG otherwise, at the best quality that fits the server cap.
 */
export async function prepareAvatarImage(file: Blob): Promise<{ mime: AvatarImageMime; data: string }> {
  if (file.size > AVATAR_SOURCE_MAX_BYTES) throw new AvatarImageError("too_large");
  if (sniffImage(new Uint8Array(await file.slice(0, 12).arrayBuffer())) === null) throw new AvatarImageError("unsupported");
  let bitmap: ImageBitmap;
  try {
    bitmap = await createImageBitmap(file);
  } catch {
    throw new AvatarImageError("unreadable");
  }
  try {
    const canvas = document.createElement("canvas");
    canvas.width = AVATAR_SIZE;
    canvas.height = AVATAR_SIZE;
    const context = canvas.getContext("2d");
    if (!context) throw new AvatarImageError("unreadable");
    const { x, y, side } = coverSquare(bitmap.width, bitmap.height);
    context.drawImage(bitmap, x, y, side, side, 0, 0, AVATAR_SIZE, AVATAR_SIZE);
    for (const quality of [0.85, 0.7, 0.5, 0.3]) {
      const blob = (await canvasBlob(canvas, "image/webp", quality)) ?? (await canvasBlob(canvas, "image/jpeg", quality));
      if (!blob) throw new AvatarImageError("unreadable");
      if (blob.size > DESKTOP_AVATAR_MAX_BYTES) continue;
      const bytes = new Uint8Array(await blob.arrayBuffer());
      const mime = sniffImage(bytes);
      // The browser may answer a PNG when it cannot encode WebP; it is still one of the three formats.
      if (mime === null) throw new AvatarImageError("unreadable");
      return { mime, data: bytesToBase64(bytes) };
    }
    throw new AvatarImageError("too_large");
  } finally {
    bitmap.close();
  }
}
