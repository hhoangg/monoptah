// The commands of the tab strips, the pane drop zones and the keyboard shortcuts. Each one changes the workspace of the
// desktop and then points the URL at the focused pane's active tab: the focused pane drives the URL.
import { useRouter } from "@tanstack/react-router";
import { useLayoutEffect, useMemo, useRef } from "react";
import { useCatalog } from "@/data";
import {
  activateTab,
  canSplit,
  closeTab,
  focusAdjacentPane,
  focusPane,
  moveTab,
  parseTab,
  removePane,
  resizeSplit,
  splitPane,
  type SplitDir,
  type TabKey,
} from "./paneTree";
import { getWorkspace, updateWorkspace } from "./tabsStore";
import { tabLocation, tabOfPath } from "./tabRoutes";

export interface Size {
  width: number;
  height: number;
}

export interface TabActions {
  activate: (paneId: string, key: TabKey) => void;
  focus: (paneId: string) => void;
  close: (paneId: string, key: TabKey) => void;
  /** Closes a pane and its tabs. */
  closePane: (paneId: string) => void;
  /** Adds an empty pane beside the focused one and opens the New session page in it. */
  split: (dir: SplitDir) => void;
  /** Whether a split of `paneId` (default: the focused pane) in `dir` would fit. */
  canSplit: (dir: SplitDir, paneId?: string) => boolean;
  /** Moves a tab into a pane (at an index) or, with `edge`, into a new pane beside it. */
  drop: (key: TabKey, target: { paneId: string; index?: number; edge?: { dir: SplitDir; place: "before" | "after" } }) => void;
  /** The tab `step` places from the active one in the focused pane (wraps). */
  cycle: (step: 1 | -1) => void;
  /** The `n`th tab of the focused pane, 0 based; -1 is the last. */
  activateIndex: (n: number) => void;
  closeActive: () => void;
  focusAdjacent: (step: 1 | -1) => void;
  resize: (path: number[], sizes: number[], commit: boolean) => void;
}

export function useTabActions(desktopId: string, size: Size | null): TabActions {
  const router = useRouter();
  const catalog = useCatalog().data;
  // The commands are read at call time (a keyboard handler outlives the render that made it).
  const live = useRef({ catalog, size });
  useLayoutEffect(() => {
    live.current = { catalog, size };
  });

  return useMemo<TabActions>(() => {
    const urlKey = () => tabOfPath(router.state.location.pathname);

    /** Points the URL at the active tab of the focused pane (the user chose a tab or a pane). An empty pane leaves the URL alone. */
    const sync = () => {
      const ws = getWorkspace(desktopId);
      const key = ws.panes[ws.focused]?.active ?? null;
      if (!key || key === urlKey()) return;
      const location = tabLocation(desktopId, key);
      if (location) void router.navigate(location);
    };

    /**
     * After tabs left the workspace: only when the URL pointed at one of them does it move on, to the focused pane's
     * active tab, else to the project (or the workspace home). A page that is not a tab (New session, Files, Git...)
     * stays where it is.
     */
    const leaveClosed = (closed: Array<TabKey | undefined>) => {
      const current = urlKey();
      if (!current || !closed.includes(current)) return;
      const ws = getWorkspace(desktopId);
      const next = ws.panes[ws.focused]?.active ?? null;
      const location = next ? tabLocation(desktopId, next) : null;
      if (location) {
        void router.navigate(location);
        return;
      }
      const tab = parseTab(current);
      const projectId = tab?.kind === "session" ? live.current.catalog?.sessions.find((session) => session.id === tab.id)?.projectId : undefined;
      void router.navigate(projectId ? { to: "/d/$desktopId/p/$projectId", params: { desktopId, projectId } } : { to: "/d/$desktopId", params: { desktopId } });
    };

    const update = (change: Parameters<typeof updateWorkspace>[1]) => updateWorkspace(desktopId, change);
    const focusedPane = () => {
      const ws = getWorkspace(desktopId);
      return ws.panes[ws.focused];
    };

    const actions: TabActions = {
      activate: (paneId, key) => {
        update((ws) => activateTab(ws, paneId, key));
        sync();
      },
      focus: (paneId) => {
        update((ws) => focusPane(ws, paneId));
        sync();
      },
      close: (paneId, key) => {
        update((ws) => closeTab(ws, paneId, key));
        leaveClosed([key]);
      },
      closePane: (paneId) => {
        const before = getWorkspace(desktopId);
        const keys = before.panes[paneId]?.tabs ?? [];
        // The focused pane shows the page of the URL when it is not a tab: that page goes with the pane.
        const showedPage = before.focused === paneId && urlKey() === null;
        update((ws) => removePane(ws, paneId));
        if (showedPage) sync();
        else leaveClosed(keys);
      },
      canSplit: (dir, paneId) => canSplit(getWorkspace(desktopId), dir, live.current.size, paneId),
      split: (dir) => {
        if (!canSplit(getWorkspace(desktopId), dir, live.current.size)) return;
        const active = focusedPane()?.active;
        const tab = active ? parseTab(active) : null;
        const projectId = tab?.kind === "session" ? live.current.catalog?.sessions.find((session) => session.id === tab.id)?.projectId : undefined;
        update((ws) => splitPane(ws, ws.focused, dir));
        // Like the native Split: a new session beside the focused one.
        if (projectId) void router.navigate({ to: "/d/$desktopId/p/$projectId/new", params: { desktopId, projectId } });
      },
      drop: (key, { paneId, index, edge }) => {
        const ownerOf = () => Object.values(getWorkspace(desktopId).panes).find((pane) => pane.tabs.includes(key))?.id;
        const before = ownerOf();
        if (!edge) {
          update((ws) => moveTab(ws, key, paneId, index));
        } else {
          update((ws) => {
            const split = splitPane(ws, paneId, edge.dir, edge.place);
            // The room is measured for the pane the tab is dropped on, not for the focused one.
            if (split === ws || !canSplit(ws, edge.dir, live.current.size, paneId)) return moveTab(ws, key, paneId, index);
            return moveTab(split, key, split.focused);
          });
        }
        // A tab that changed pane became the active one there: show it. A reorder inside a pane leaves the page alone.
        if (ownerOf() !== before) sync();
      },
      cycle: (step) => {
        const pane = focusedPane();
        if (!pane || pane.tabs.length < 2) return;
        const at = pane.active ? pane.tabs.indexOf(pane.active) : -1;
        const next = pane.tabs[(at + step + pane.tabs.length) % pane.tabs.length];
        if (next) actions.activate(pane.id, next);
      },
      activateIndex: (n) => {
        const pane = focusedPane();
        const key = pane ? pane.tabs[n < 0 ? pane.tabs.length - 1 : n] : undefined;
        if (pane && key) actions.activate(pane.id, key);
      },
      closeActive: () => {
        const key = urlKey();
        const owner = key ? Object.values(getWorkspace(desktopId).panes).find((candidate) => candidate.tabs.includes(key)) : undefined;
        if (owner && key) actions.close(owner.id, key);
      },
      focusAdjacent: (step) => {
        update((ws) => focusAdjacentPane(ws, step));
        sync();
      },
      resize: (path, sizes, commit) => {
        updateWorkspace(desktopId, (ws) => resizeSplit(ws, path, sizes), { persist: commit });
      },
    };
    return actions;
  }, [desktopId, router]);
}
