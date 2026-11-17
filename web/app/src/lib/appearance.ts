import { create } from "zustand";
import {
  ACCENTS,
  APPEARANCES,
  DEFAULT_ACCENT,
  DEFAULT_APPEARANCE,
  DEFAULT_THEME,
  STORAGE_KEYS,
  THEME_BACKGROUNDS,
  THEMES,
  type Accent,
  type Appearance,
  type Scheme,
  type Theme,
} from "@/styles/themes";

const DARK_QUERY = "(prefers-color-scheme: dark)";

function readKey<T extends string>(key: string, allowed: readonly T[], fallback: T): T {
  try {
    const value = window.localStorage.getItem(key);
    return allowed.includes(value as T) ? (value as T) : fallback;
  } catch {
    return fallback; // storage blocked
  }
}

function writeKey(key: string, value: string): void {
  try {
    window.localStorage.setItem(key, value);
  } catch {
    // storage blocked or full: the preference stays in memory for this tab
  }
}

function systemPrefersDark(): boolean {
  return typeof window.matchMedia === "function" ? window.matchMedia(DARK_QUERY).matches : true;
}

/** Resolves the stored appearance to the scheme applied on <html>. */
export function resolveScheme(appearance: Appearance, systemDark: boolean): Scheme {
  if (appearance === "light") return "light";
  if (appearance === "dark") return "dark";
  return systemDark ? "dark" : "light";
}

/** Sets data-scheme, data-theme, data-accent and the theme-color meta, as theme-init.js does. */
export function applyAppearance(state: { appearance: Appearance; theme: Theme; accent: Accent }): Scheme {
  const scheme = resolveScheme(state.appearance, systemPrefersDark());
  const root = document.documentElement;
  root.setAttribute("data-scheme", scheme);
  root.setAttribute("data-theme", state.theme);
  root.setAttribute("data-accent", state.accent);
  const meta = document.querySelector('meta[name="theme-color"]');
  meta?.setAttribute("content", THEME_BACKGROUNDS[state.theme][scheme === "dark" ? 0 : 1]);
  return scheme;
}

export interface AppearanceState {
  appearance: Appearance;
  theme: Theme;
  accent: Accent;
  /** The scheme currently applied (the resolved appearance). */
  scheme: Scheme;
  setAppearance: (value: Appearance) => void;
  setTheme: (value: Theme) => void;
  setAccent: (value: Accent) => void;
  /** Re-reads localStorage and the media query (storage events, system scheme changes). */
  sync: () => void;
}

function readAll() {
  return {
    appearance: readKey(STORAGE_KEYS.appearance, APPEARANCES, DEFAULT_APPEARANCE),
    theme: readKey(STORAGE_KEYS.theme, THEMES, DEFAULT_THEME),
    accent: readKey(STORAGE_KEYS.accent, ACCENTS, DEFAULT_ACCENT),
  };
}

/** UI preference store (design-system.md 5.1 rule 2). Device-local, never synced to the server. */
export const useAppearance = create<AppearanceState>()((set, get) => {
  const initial = readAll();
  return {
    ...initial,
    scheme: resolveScheme(initial.appearance, systemPrefersDark()),
    setAppearance: (appearance) => {
      writeKey(STORAGE_KEYS.appearance, appearance);
      set({ appearance, scheme: applyAppearance({ ...get(), appearance }) });
    },
    setTheme: (theme) => {
      writeKey(STORAGE_KEYS.theme, theme);
      set({ theme, scheme: applyAppearance({ ...get(), theme }) });
    },
    setAccent: (accent) => {
      writeKey(STORAGE_KEYS.accent, accent);
      set({ accent, scheme: applyAppearance({ ...get(), accent }) });
    },
    sync: () => {
      const next = readAll();
      set({ ...next, scheme: applyAppearance(next) });
    },
  };
});

/**
 * Starts the cross-tab (`storage`) and system-scheme (media query) listeners. Returns a cleanup.
 * Call once at startup; also applies the stored values so the store and the DOM agree.
 */
export function startAppearanceSync(): () => void {
  const { sync } = useAppearance.getState();
  sync();
  const onStorage = (event: StorageEvent) => {
    if (event.key === null || Object.values<string>(STORAGE_KEYS).includes(event.key)) sync();
  };
  window.addEventListener("storage", onStorage);
  const media = typeof window.matchMedia === "function" ? window.matchMedia(DARK_QUERY) : null;
  const onMedia = () => {
    if (useAppearance.getState().appearance === "system") sync();
  };
  media?.addEventListener("change", onMedia);
  return () => {
    window.removeEventListener("storage", onStorage);
    media?.removeEventListener("change", onMedia);
  };
}
