// Desktop colors: the swatch list of design-system.md section 1 ("Desktop / Mono swatches").
// Owner: WS-M (U-01). The server stores the color as a free string (max 32 chars), so the stored
// value is only ever compared with this fixed list. What reaches CSS is the palette's own hex, never
// the stored string, and an unknown value falls back to a neutral gray.

export interface DesktopColor {
  /** The value stored on the server. */
  value: string;
  name: string;
}

export const DESKTOP_COLORS: readonly DesktopColor[] = [
  { value: "#459cf7", name: "Blue" },
  { value: "#ea603e", name: "Red" },
  { value: "#f4bf25", name: "Yellow" },
  { value: "#39c363", name: "Green" },
  { value: "#e45a9f", name: "Pink" },
  { value: "#b168d4", name: "Purple" },
  { value: "#37beb4", name: "Teal" },
  { value: "#ef8c39", name: "Orange" },
  { value: "#8b63e9", name: "Violet" },
];

/** The fallback for a missing or unknown color (the dark `muted` token value). */
export const FALLBACK_COLOR: DesktopColor = { value: "#8a8a8a", name: "Gray" };

/** The palette entry of a stored color (case-insensitive), or null when it is not in the palette. */
export function findDesktopColor(stored: string | null | undefined): DesktopColor | null {
  if (!stored) return null;
  const wanted = stored.trim().toLowerCase();
  return DESKTOP_COLORS.find((color) => color.value === wanted) ?? null;
}

/** Always a palette entry or the fallback: safe to put `.value` into a style. */
export function resolveDesktopColor(stored: string | null | undefined): DesktopColor {
  return findDesktopColor(stored) ?? FALLBACK_COLOR;
}
