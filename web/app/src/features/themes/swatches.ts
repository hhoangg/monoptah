import { THEME_BACKGROUNDS, type Accent, type Scheme, type Theme } from "@/styles/themes";

// Picker swatch colours. A swatch must show a theme or accent that is not the one applied to the page,
// so it cannot read the live CSS variables; these mirror tokens.css (swatches.test.ts compares them).

/** Accent each theme brings, as [dark, light] (design-system.md 1.2). */
export const THEME_ACCENT_COLORS: Record<Theme, readonly [dark: string, light: string]> = {
  mono: ["#4c9ef5", "#1f7ae0"],
  ocean: ["#5aa9f7", "#1f6fc4"],
  grove: ["#4cc38a", "#13804f"],
  ember: ["#f0884a", "#c2531a"],
  iris: ["#9d87f5", "#6a4fd6"],
  rose: ["#ec6fa3", "#c42a6c"],
};

/** Colour accents, as [dark, light] (design-system.md 1.3). "theme" is not listed: it follows the theme. */
export const ACCENT_COLORS: Record<Exclude<Accent, "theme">, readonly [dark: string, light: string]> = {
  blue: ["#4c9ef5", "#1f7ae0"],
  teal: ["#3cc4c4", "#0f7f86"],
  green: ["#4cc38a", "#13804f"],
  orange: ["#f0884a", "#c2531a"],
  red: ["#f06a6a", "#c9302c"],
  pink: ["#ec6fa3", "#c42a6c"],
  violet: ["#9d87f5", "#6a4fd6"],
};

export const ACCENT_LABELS: Record<Accent, string> = {
  theme: "Theme default",
  blue: "Blue",
  teal: "Teal",
  green: "Green",
  orange: "Orange",
  red: "Red",
  pink: "Pink",
  violet: "Violet",
};

const index = (scheme: Scheme) => (scheme === "dark" ? 0 : 1);

export function themeSwatch(theme: Theme, scheme: Scheme): { background: string; accent: string } {
  return { background: THEME_BACKGROUNDS[theme][index(scheme)], accent: THEME_ACCENT_COLORS[theme][index(scheme)] };
}

export function accentSwatch(accent: Accent, theme: Theme, scheme: Scheme): string {
  return accent === "theme" ? THEME_ACCENT_COLORS[theme][index(scheme)] : ACCENT_COLORS[accent][index(scheme)];
}
