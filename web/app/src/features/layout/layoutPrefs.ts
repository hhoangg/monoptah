// Persisted layout preferences of the app shell (design-system.md section 11), kept in localStorage:
//   monoptah.layout.sidebarCollapsed  "1" | "0"   tablet sidebar collapsed to a rail
//   monoptah.layout.sidebarWidth      px, clamped to 320-360 (desktop, resizable)
//   monoptah.layout.panelWidth        px, clamped to 380-480 (docked right panel, resizable)
import { create } from "zustand";

export const SIDEBAR_COLLAPSED_KEY = "monoptah.layout.sidebarCollapsed";
export const SIDEBAR_WIDTH_KEY = "monoptah.layout.sidebarWidth";
export const PANEL_WIDTH_KEY = "monoptah.layout.panelWidth";

export const SIDEBAR_WIDTH = { min: 320, max: 360, default: 320 } as const;
export const PANEL_WIDTH = { min: 380, max: 480, default: 420 } as const;
/** The tablet sidebar is fixed (design-system.md section 11). */
export const TABLET_SIDEBAR_WIDTH = 300;
/** The tablet right panel, an overlay. */
export const TABLET_PANEL_WIDTH = 360;

type Range = { readonly min: number; readonly max: number; readonly default: number };

export function clampWidth(range: Range, value: number): number {
  if (!Number.isFinite(value)) return range.default;
  return Math.min(range.max, Math.max(range.min, Math.round(value)));
}

function read(key: string): string | null {
  try {
    return window.localStorage.getItem(key);
  } catch {
    return null; // storage blocked
  }
}

function write(key: string, value: string): void {
  try {
    window.localStorage.setItem(key, value);
  } catch {
    // storage blocked or full: the preference stays in memory for this tab
  }
}

function readWidth(key: string, range: Range): number {
  const raw = read(key);
  return raw === null ? range.default : clampWidth(range, Number(raw));
}

interface LayoutPrefsState {
  sidebarCollapsed: boolean;
  sidebarWidth: number;
  panelWidth: number;
  setSidebarCollapsed: (collapsed: boolean) => void;
  setSidebarWidth: (width: number) => void;
  setPanelWidth: (width: number) => void;
}

function load(): Pick<LayoutPrefsState, "sidebarCollapsed" | "sidebarWidth" | "panelWidth"> {
  return {
    sidebarCollapsed: read(SIDEBAR_COLLAPSED_KEY) === "1",
    sidebarWidth: readWidth(SIDEBAR_WIDTH_KEY, SIDEBAR_WIDTH),
    panelWidth: readWidth(PANEL_WIDTH_KEY, PANEL_WIDTH),
  };
}

export const useLayoutPrefs = create<LayoutPrefsState>((set) => ({
  ...load(),
  setSidebarCollapsed: (collapsed) => {
    write(SIDEBAR_COLLAPSED_KEY, collapsed ? "1" : "0");
    set({ sidebarCollapsed: collapsed });
  },
  setSidebarWidth: (width) => {
    const next = clampWidth(SIDEBAR_WIDTH, width);
    write(SIDEBAR_WIDTH_KEY, String(next));
    set({ sidebarWidth: next });
  },
  setPanelWidth: (width) => {
    const next = clampWidth(PANEL_WIDTH, width);
    write(PANEL_WIDTH_KEY, String(next));
    set({ panelWidth: next });
  },
}));

/** Re-reads the stored values (for tests, after seeding or clearing localStorage). */
export function reloadLayoutPrefs(): void {
  useLayoutPrefs.setState(load());
}
