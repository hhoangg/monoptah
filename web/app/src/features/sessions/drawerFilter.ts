// F-12: the drawer's filter and kind choice, kept in localStorage so they survive a reload.
import { useCallback, useState } from "react";

export const DRAWER_FILTER_KEY = "monoptah.drawer.filter";

export type DrawerFilter = "all" | "unread" | "working";
/** Which kinds the list shows. */
export type DrawerShow = "both" | "monos" | "projects";

export interface DrawerFilterState {
  filter: DrawerFilter;
  show: DrawerShow;
}

export const DEFAULT_DRAWER_FILTER: DrawerFilterState = { filter: "all", show: "both" };

const FILTERS: readonly string[] = ["all", "unread", "working"];
const SHOWS: readonly string[] = ["both", "monos", "projects"];

export function readDrawerFilter(): DrawerFilterState {
  try {
    const raw = window.localStorage.getItem(DRAWER_FILTER_KEY);
    const parsed = raw ? (JSON.parse(raw) as Partial<Record<keyof DrawerFilterState, unknown>>) : null;
    if (!parsed) return DEFAULT_DRAWER_FILTER;
    return {
      filter: typeof parsed.filter === "string" && FILTERS.includes(parsed.filter) ? (parsed.filter as DrawerFilter) : "all",
      show: typeof parsed.show === "string" && SHOWS.includes(parsed.show) ? (parsed.show as DrawerShow) : "both",
    };
  } catch {
    return DEFAULT_DRAWER_FILTER;
  }
}

export function writeDrawerFilter(state: DrawerFilterState): void {
  try {
    window.localStorage.setItem(DRAWER_FILTER_KEY, JSON.stringify(state));
  } catch {
    // Storage blocked or full: the choice lasts until the page reloads.
  }
}

/** The drawer's filter state, persisted on every change. */
export function useDrawerFilter(): [DrawerFilterState, (patch: Partial<DrawerFilterState>) => void] {
  const [state, setState] = useState<DrawerFilterState>(readDrawerFilter);
  const update = useCallback((patch: Partial<DrawerFilterState>) => {
    setState((previous) => {
      const next = { ...previous, ...patch };
      writeDrawerFilter(next);
      return next;
    });
  }, []);
  return [state, update];
}
