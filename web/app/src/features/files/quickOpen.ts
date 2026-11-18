// Quick open (Mod+P): the shared open state, the host bookkeeping and the shortcut handler.
// The dialog (QuickOpen.tsx) is mounted by whichever Files surface is on screen (the files route or the
// right panel); only the most recently mounted host renders it. With no host on screen, Mod+P opens the
// Files panel for the current project, which brings its own host. The project is the one the shell shows
// (the route's, or the session's from the catalog), else the one in the URL; never a stale panel or an
// earlier project. A pending target is dropped when no host shows it any more.
import { useEffect, useId } from "react";
import { create } from "zustand";
import { openPanel, usePanels, type PanelTarget } from "@/features/layout/panels";
import { registerShortcut } from "@/features/layout/shortcuts";
import { sessionCan } from "@/ui/hooks";
import { getLinkState } from "@/link";

interface QuickOpenState {
  /** The project the dialog searches, or null while closed. */
  target: PanelTarget | null;
  /** Mounted hosts, oldest first. */
  hosts: string[];
}

export const useQuickOpen = create<QuickOpenState>(() => ({ target: null, hosts: [] }));

export function closeQuickOpen(): void {
  useQuickOpen.setState({ target: null });
}

/** Registers a mounted Files surface for `target`. Returns whether this surface renders the dialog. */
export function useQuickOpenHost(target: PanelTarget): boolean {
  const id = useId();
  const { desktopId, projectId } = target;
  useEffect(() => {
    useQuickOpen.setState((state) => {
      const other = state.target && (state.target.desktopId !== desktopId || state.target.projectId !== projectId);
      return { hosts: [...state.hosts, id], target: other ? null : state.target };
    });
    return () =>
      useQuickOpen.setState((state) => {
        const hosts = state.hosts.filter((host) => host !== id);
        // No surface is left to show the dialog: a later surface must not pop it up unasked.
        return { hosts, target: hosts.length === 0 ? null : state.target };
      });
  }, [id, desktopId, projectId]);
  return useQuickOpen((state) => state.hosts.at(-1) === id);
}

const PROJECT_PATH = /^\/d\/([^/]+)\/p\/([^/]+)/;

/** The project Mod+P searches: the shell's scope, else a project route in the URL, else none. */
export function currentProject(pathname: string = typeof window === "undefined" ? "" : window.location.pathname): PanelTarget | undefined {
  const { scope } = usePanels.getState();
  if (scope) return { desktopId: scope.desktopId, projectId: scope.projectId };
  const match = PROJECT_PATH.exec(pathname);
  if (match?.[1] && match[2]) return { desktopId: decodeURIComponent(match[1]), projectId: decodeURIComponent(match[2]) };
  return undefined;
}

/** The Mod+P handler. */
export function startQuickOpen(): void {
  const target = currentProject();
  if (!target) return;
  const { scope, scopeTarget } = usePanels.getState();
  // The session's target only counts for the project it belongs to (the scope).
  const sessionTarget = scope && scope.desktopId === target.desktopId && scope.projectId === target.projectId ? scopeTarget : null;
  const context = { target: sessionTarget, desktopCapabilities: getLinkState(target.desktopId).capabilities };
  if (!sessionCan("files", context)) return;
  if (useQuickOpen.getState().hosts.length === 0 && !openPanel("files", target, context)) return;
  useQuickOpen.setState({ target });
}

let registered = false;
/** Registers the "quick-open" shortcut once. */
export function registerQuickOpen(): void {
  if (registered) return;
  registered = true;
  registerShortcut("quick-open", startQuickOpen);
}
