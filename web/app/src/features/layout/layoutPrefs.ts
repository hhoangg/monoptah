// Persisted layout preferences of the app shell (design-system.md section 11), kept in localStorage:
//   monoptah.layout.sidebarCollapsed  "1" | "0"   tablet sidebar collapsed to a rail
//   monoptah.layout.sidebarWidth      px, clamped to 320-360 (desktop, resizable)
//   monoptah.layout.sessionListWidth px, clamped to 240-560 (desktop, resizable: the project's session list, as in the native classic layout)
//   monoptah.layout.panelWidth        px, clamped to 380-480 (docked right panel, resizable)
//   monoptah.layout.mode              "classic" | "focused" (the native Appearance > Layout picker): classic shows the
//                                     session-list column from 1024px, focused leaves it out (the project page is its own screen)
import { create } from "zustand";

export const SIDEBAR_COLLAPSED_KEY = "monoptah.layout.sidebarCollapsed";
export const SIDEBAR_WIDTH_KEY = "monoptah.layout.sidebarWidth";
export const SESSION_LIST_WIDTH_KEY = "monoptah.layout.sessionListWidth";
export const PANEL_WIDTH_KEY = "monoptah.layout.panelWidth";
export const LAYOUT_MODE_KEY = "monoptah.layout.mode";

export type LayoutMode = "classic" | "focused";
export const LAYOUT_MODES: readonly LayoutMode[] = ["classic", "focused"];

export const SIDEBAR_WIDTH = { min: 320, max: 360, default: 320 } as const;
/**
 * The session-list column beside the sidebar from 1024px, resizable like the native classic sidebar (native: 260
 * default, 200-560). The web session card carries more (branch, model, status, menu) than the native compact card, so
 * the default is wider and the minimum higher.
 */
export const SESSION_LIST_WIDTH = { min: 240, max: 560, default: 320 } as const;
export const PANEL_WIDTH = { min: 380, max: 480, default: 420 } as const;
/** The main pane never gets narrower than this while a panel is docked. */
export const MIN_MAIN_WIDTH = 320;
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

/** A stored width: digits only, clamped to the range; anything else (empty, text, negative) is the default. */
function readWidth(key: string, range: Range): number {
  const raw = read(key);
  return raw !== null && /^\d+$/.test(raw) ? clampWidth(range, Number(raw)) : range.default;
}

/**
 * The widest the session list may be at `viewport` px when `taken` px go to the sidebar and a docked panel: the chat
 * keeps `MIN_MAIN_WIDTH` (the native sidebar is capped at half the window, this is the same idea for a narrower web
 * window). The list is the pane that gives way: it shrinks to `SESSION_LIST_WIDTH.min` before the panel narrows.
 */
export function sessionListMax(viewport: number, taken: number): number {
  return Math.max(SESSION_LIST_WIDTH.min, Math.min(SESSION_LIST_WIDTH.max, viewport - taken - MIN_MAIN_WIDTH));
}

/**
 * The widest a docked panel may be at `viewport` px with a `sidebar` px sidebar, keeping `MIN_MAIN_WIDTH`
 * for the main pane; null when even the narrowest panel (380) does not fit, so the shell uses the overlay.
 */
export function dockedPanelMax(viewport: number, sidebar: number): number | null {
  const room = viewport - sidebar - MIN_MAIN_WIDTH;
  return room < PANEL_WIDTH.min ? null : Math.min(PANEL_WIDTH.max, room);
}

interface LayoutPrefsState {
  sidebarCollapsed: boolean;
  sidebarWidth: number;
  sessionListWidth: number;
  panelWidth: number;
  layoutMode: LayoutMode;
  setLayoutMode: (mode: LayoutMode) => void;
  setSidebarCollapsed: (collapsed: boolean) => void;
  setSidebarWidth: (width: number) => void;
  setSessionListWidth: (width: number) => void;
  setPanelWidth: (width: number) => void;
  /** Widths change in memory while dragging; these write the current value to localStorage. */
  persistSidebarWidth: () => void;
  persistSessionListWidth: () => void;
  persistPanelWidth: () => void;
}

/** A stored layout mode; anything but "focused" (missing, unknown) is the classic layout the web shipped with. */
function readMode(): LayoutMode {
  return read(LAYOUT_MODE_KEY) === "focused" ? "focused" : "classic";
}

function load(): Pick<LayoutPrefsState, "sidebarCollapsed" | "sidebarWidth" | "sessionListWidth" | "panelWidth" | "layoutMode"> {
  return {
    layoutMode: readMode(),
    sidebarCollapsed: read(SIDEBAR_COLLAPSED_KEY) === "1",
    sidebarWidth: readWidth(SIDEBAR_WIDTH_KEY, SIDEBAR_WIDTH),
    sessionListWidth: readWidth(SESSION_LIST_WIDTH_KEY, SESSION_LIST_WIDTH),
    panelWidth: readWidth(PANEL_WIDTH_KEY, PANEL_WIDTH),
  };
}

export const useLayoutPrefs = create<LayoutPrefsState>((set, get) => ({
  ...load(),
  setLayoutMode: (mode) => {
    write(LAYOUT_MODE_KEY, mode);
    set({ layoutMode: mode });
  },
  setSidebarCollapsed: (collapsed) => {
    write(SIDEBAR_COLLAPSED_KEY, collapsed ? "1" : "0");
    set({ sidebarCollapsed: collapsed });
  },
  setSidebarWidth: (width) => set({ sidebarWidth: clampWidth(SIDEBAR_WIDTH, width) }),
  setSessionListWidth: (width) => set({ sessionListWidth: clampWidth(SESSION_LIST_WIDTH, width) }),
  setPanelWidth: (width) => set({ panelWidth: clampWidth(PANEL_WIDTH, width) }),
  persistSidebarWidth: () => write(SIDEBAR_WIDTH_KEY, String(get().sidebarWidth)),
  persistSessionListWidth: () => write(SESSION_LIST_WIDTH_KEY, String(get().sessionListWidth)),
  persistPanelWidth: () => write(PANEL_WIDTH_KEY, String(get().panelWidth)),
}));

/** Re-reads the stored values (for tests, after seeding or clearing localStorage). */
export function reloadLayoutPrefs(): void {
  useLayoutPrefs.setState(load());
}
