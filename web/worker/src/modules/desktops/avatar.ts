// The avatar of a desktop (migration 0008): an emoji, or a small PNG, JPEG or WebP the browser re-encoded. The Worker does
// not trust the browser: it decodes the base64, checks the size and the magic bytes itself, and serves the bytes with a
// content type taken from those bytes, never from what the client claimed. SVG is never accepted (it can carry script).
import { DESKTOP_AVATAR_MAX_BYTES, type DesktopAvatar } from "@monoptah/protocol";
import { toHex } from "../../lib/crypto.ts";

export type AvatarMime = "image/png" | "image/jpeg" | "image/webp";

const startsWith = (bytes: Uint8Array, prefix: readonly number[], offset = 0): boolean => prefix.every((value, index) => bytes[offset + index] === value);

/** The format of `bytes` by its magic bytes, or null for anything else (SVG, GIF, HTML, a truncated file). */
export function sniffImage(bytes: Uint8Array): AvatarMime | null {
  if (bytes.length >= 8 && startsWith(bytes, [0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a])) return "image/png";
  if (bytes.length >= 3 && startsWith(bytes, [0xff, 0xd8, 0xff])) return "image/jpeg";
  // RIFF <size> WEBP
  if (bytes.length >= 12 && startsWith(bytes, [0x52, 0x49, 0x46, 0x46]) && startsWith(bytes, [0x57, 0x45, 0x42, 0x50], 8)) return "image/webp";
  return null;
}

/** Decodes standard base64; null when it is not valid. */
export function decodeBase64(text: string): Uint8Array | null {
  try {
    const binary = atob(text);
    const bytes = new Uint8Array(binary.length);
    for (let index = 0; index < binary.length; index += 1) bytes[index] = binary.charCodeAt(index);
    return bytes;
  } catch {
    return null;
  }
}

/** The first 16 hex characters of the SHA-256 of the image: the cache key of its URL. */
export async function imageHash(bytes: Uint8Array): Promise<string> {
  return toHex(new Uint8Array(await crypto.subtle.digest("SHA-256", bytes))).slice(0, 16);
}

export interface ImageAvatar {
  hash: string;
  /** The base64 that is stored: the client's text, which decodes to exactly the checked bytes. */
  data: string;
}

/** Checks the base64 of an uploaded image: valid, within the size cap and a PNG, JPEG or WebP by its magic bytes. Null refuses it. */
export async function checkImage(base64: string): Promise<ImageAvatar | null> {
  const bytes = decodeBase64(base64);
  if (!bytes || bytes.length === 0 || bytes.length > DESKTOP_AVATAR_MAX_BYTES) return null;
  if (sniffImage(bytes) === null) return null;
  return { hash: await imageHash(bytes), data: base64 };
}

export interface AvatarColumns {
  avatar_kind: string | null;
  avatar_emoji: string | null;
  avatar_hash: string | null;
}

/** The avatar a summary shows, from the row's columns (null for initials, or for a row whose columns do not agree). */
export function avatarOf(row: AvatarColumns): DesktopAvatar | null {
  if (row.avatar_kind === "emoji" && row.avatar_emoji) return { kind: "emoji", emoji: row.avatar_emoji };
  if (row.avatar_kind === "image" && row.avatar_hash) return { kind: "image", hash: row.avatar_hash };
  return null;
}
