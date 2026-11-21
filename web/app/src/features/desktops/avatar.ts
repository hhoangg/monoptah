// The avatar of a desktop in the activity rail: initials, a deterministic color and a readable text color.
// The colors are the desktop swatches of design-system.md section 1 (colors.ts), never free-form values.
import { DESKTOP_COLORS, findDesktopColor } from "./colors";

/** Up to two initials: the first letters of the first two words ("Studio Mac" is "SM", "Linux" is "L"). "?" when empty. */
export function desktopInitials(name: string): string {
  const words = name.split(/[^\p{L}\p{N}]+/u).filter((word) => word !== "");
  const letters = words.slice(0, 2).map((word) => Array.from(word)[0] ?? "");
  const text = letters.join("").toUpperCase();
  return text === "" ? "?" : text;
}

/** A stable non-negative hash of a string (FNV-1a, 32 bit). */
function hash(text: string): number {
  let value = 0x811c9dc5;
  for (let index = 0; index < text.length; index += 1) {
    value ^= text.charCodeAt(index);
    value = Math.imul(value, 0x01000193) >>> 0;
  }
  return value;
}

/**
 * The avatar background: the color the user chose for the desktop when it is one of the swatches, else a swatch picked
 * from the desktop id, so the same desktop always has the same color on every device.
 */
export function desktopAvatarColor(id: string, stored: string | null | undefined): string {
  const chosen = findDesktopColor(stored);
  if (chosen) return chosen.value;
  return DESKTOP_COLORS[hash(id) % DESKTOP_COLORS.length]!.value;
}

function luminance(hex: string): number {
  const channel = (offset: number) => {
    const value = parseInt(hex.slice(offset, offset + 2), 16) / 255;
    return value <= 0.03928 ? value / 12.92 : ((value + 0.055) / 1.055) ** 2.4;
  };
  return 0.2126 * channel(1) + 0.7152 * channel(3) + 0.0722 * channel(5);
}

const DARK_TEXT = 0.0037; // luminance of --on-swatch-dark (#0c0c0c)

/**
 * The token of the text color that has the higher WCAG contrast on a swatch. Every desktop swatch has 4.5:1 or more with
 * the dark one (axe color-contrast), so the light token only wins on a dark custom color.
 */
export function onAvatarColor(hex: string): string {
  const light = 1.05 / (luminance(hex) + 0.05);
  const dark = (luminance(hex) + 0.05) / (DARK_TEXT + 0.05);
  return dark >= light ? "var(--on-swatch-dark)" : "var(--on-swatch-light)";
}

/** The URL of a desktop's image avatar; the hash is the cache key (an image never changes under one hash). */
export function desktopAvatarUrl(id: string, hash: string): string {
  return `/api/desktops/${encodeURIComponent(id)}/avatar?v=${encodeURIComponent(hash)}`;
}

export type GrantCue = "pending" | "revoked" | "none";

/**
 * What the rail shows for this device's grant on a desktop: null while it is unknown or when access is confirmed.
 * Opening a desktop without a grant requests one (the link-state screen does it), so "none" says so.
 */
export function grantCue(grants: ReadonlyArray<{ deviceId: string; state: string }> | undefined, deviceId: string | null): GrantCue | null {
  if (!grants || !deviceId) return null;
  const mine = grants.find((item) => item.deviceId === deviceId);
  if (!mine) return "none";
  if (mine.state === "pending") return "pending";
  if (mine.state === "revoked") return "revoked";
  return null;
}

export const GRANT_CUE_LABEL: Record<GrantCue, string> = {
  pending: "waiting for confirmation on the desktop",
  revoked: "access revoked",
  none: "no access yet, open it to request",
};
