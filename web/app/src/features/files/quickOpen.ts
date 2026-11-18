// Quick open (Mod+P): the shared open state, the host bookkeeping and the shortcut handler.
// The dialog (QuickOpen.tsx) is mounted by whichever Files surface is on screen (the files route or the
// right panel); only the most recently mounted host renders it. With no host on screen, Mod+P opens the
// Files panel for the current project, which brings its own host.
import { useEffect, useId } from "react";
import { create } from "zustand";
import { openPanel, usePanels, type PanelTarget } from "@/features/layout/panels";
import { registerShortcut } from "@/features/layout/shortcuts";
import { getLinkState } from "@/link";

interface QuickOpenState {
  /** The project the dialog searches, or null while closed. */
  target: PanelTarget | null;
  /** Mounted hosts, oldest first. */
  hosts: string[];
}

export const useQuickOpen = create<QuickOpenState>(() => ({ target: null, hosts: [] }));

/** The last project a Files surface showed: Mod+P from another screen of the same app session reuses it. */
let lastTarget: PanelTarget | undefined;

export function closeQuickOpen(): void {
  useQuickOpen.setState({ target: null });
}

/** Registers a mounted Files surface for `target`. Returns whether this surface renders the dialog. */
export function useQuickOpenHost(target: PanelTarget): boolean {
  const id = useId();
  const { desktopId, projectId } = target;
  useEffect(() => {
    lastTarget = { desktopId, projectId };
    useQuickOpen.setState((state) => ({ hosts: [...state.hosts, id] }));
    return () => useQuickOpen.setState((state) => ({ hosts: state.hosts.filter((host) => host !== id) }));
  }, [id, desktopId, projectId]);
  return useQuickOpen((state) => state.hosts.at(-1) === id);
}

const PROJECT_PATH = /^\/d\/([^/]+)\/p\/([^/]+)/;

/** The project Mod+P searches: the open panel, a project route, or the last one shown. */
export function currentProject(pathname: string = typeof window === "undefined" ? "" : window.location.pathname): PanelTarget | undefined {
  const open = usePanels.getState().open;
  if (open) return { desktopId: open.desktopId, projectId: open.projectId };
  const match = PROJECT_PATH.exec(pathname);
  if (match?.[1] && match[2]) return { desktopId: decodeURIComponent(match[1]), projectId: decodeURIComponent(match[2]) };
  return lastTarget;
}

/** The Mod+P handler. */
export function startQuickOpen(): void {
  const target = currentProject();
  if (!target) return;
  const context = { target: null, desktopCapabilities: getLinkState(target.desktopId).capabilities };
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
