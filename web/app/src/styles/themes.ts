// Appearance vocabulary of design-system.md 5.1. The values are persisted in localStorage under
// monoptah.appearance, monoptah.theme and monoptah.accent and read by public/theme-init.js.

export const APPEARANCES = ["system", "light", "dark"] as const;
export type Appearance = (typeof APPEARANCES)[number];

export const THEMES = ["mono", "ocean", "grove", "ember", "iris", "rose"] as const;
export type Theme = (typeof THEMES)[number];

export const ACCENTS = ["theme", "blue", "teal", "green", "orange", "red", "pink", "violet"] as const;
export type Accent = (typeof ACCENTS)[number];

export type Scheme = "light" | "dark";

export const DEFAULT_APPEARANCE: Appearance = "system";
export const DEFAULT_THEME: Theme = "mono";
export const DEFAULT_ACCENT: Accent = "theme";

export const STORAGE_KEYS = {
  appearance: "monoptah.appearance",
  theme: "monoptah.theme",
  accent: "monoptah.accent",
} as const;

/** `background` per theme as [dark, light]; used for <meta name="theme-color">. Matches tokens.css. */
export const THEME_BACKGROUNDS: Record<Theme, readonly [dark: string, light: string]> = {
  mono: ["#131313", "#f7f7f7"],
  ocean: ["#0e1216", "#f3f6f9"],
  grove: ["#0e1612", "#f3f9f6"],
  ember: ["#16110e", "#f9f5f3"],
  iris: ["#110e16", "#f5f3f9"],
  rose: ["#160e11", "#f9f3f5"],
};

/** Display names of the tinted themes (design-system.md 1.2). */
export const THEME_LABELS: Record<Theme, string> = {
  mono: "Classic",
  ocean: "Cobalt",
  grove: "Jade",
  ember: "Copper",
  iris: "Violet",
  rose: "Blush",
};
