// The project tools the shell offers for the project of the current screen: the Files, Changes and Git panels and
// the background picker. One hook feeds every entry point (the project bar and the command palette), so they
// show and hide the same things. Each panel is offered only when its kind is registered and the
// session-capabilities predicate allows it for the session's target and the desktop's capabilities.
import { useRouterState } from "@tanstack/react-router";
import { create } from "zustand";
import { useLinkState } from "@/link";
import { sessionCan, type SessionCapabilityContext } from "@/ui/hooks";
import { PANEL_KINDS, hasPanel, openPanel, panelMatchesScope, togglePanel, usePanels, type PanelKind, type PanelTarget } from "./panels";

/** The project whose background dialog is open (null while closed). */
export const useBackgroundDialog = create<{ target: PanelTarget | null }>(() => ({ target: null }));

export function openBackgroundDialog(target: PanelTarget): void {
  useBackgroundDialog.setState({ target });
}
export function closeBackgroundDialog(): void {
  useBackgroundDialog.setState({ target: null });
}

export const PANEL_LABELS: Record<PanelKind, string> = { files: "Files", changes: "Changes", git: "Git" };

const ROUTE_TOOL = /\/p\/[^/]+\/(files|changes|git)(?:\/|$)/;

/** The tool whose full screen is the current route (Files, Changes or Git), else null. Its panel would duplicate the screen. */
export function useRouteTool(): PanelKind | null {
  return useRouterState({
    select: (state) => {
      const match = ROUTE_TOOL.exec(state.location.pathname);
      return match ? (match[1] as PanelKind) : null;
    },
  });
}

export interface ProjectTools {
  /** The tool whose screen is the current route (shown as current, never opened as a second panel). */
  current: PanelKind | null;
  /** The project of the current screen, or null where there is none. */
  scope: PanelTarget | null;
  /** The panel kinds to offer for it (registered and allowed). */
  panels: PanelKind[];
  isOpen: (kind: PanelKind) => boolean;
  toggle: (kind: PanelKind) => void;
  open: (kind: PanelKind) => boolean;
  openBackground: () => void;
}

export function useProjectTools(): ProjectTools {
  const scope = usePanels((state) => state.scope);
  const scopeTarget = usePanels((state) => state.scopeTarget);
  const open = usePanels((state) => state.open);
  const capabilities = useLinkState((state) => state.capabilities);
  const current = useRouteTool();
  const context: SessionCapabilityContext = { target: scopeTarget, desktopCapabilities: capabilities };
  const panels = scope ? PANEL_KINDS.filter((kind) => hasPanel(kind) && sessionCan(kind, context)) : [];
  return {
    current,
    scope,
    panels,
    isOpen: (kind) => panelMatchesScope(open, scope) && open.kind === kind,
    toggle: (kind) => {
      if (scope && kind !== current) togglePanel(kind, scope, context);
    },
    open: (kind) => (scope && kind !== current ? openPanel(kind, scope, context) : false),
    openBackground: () => {
      if (scope) openBackgroundDialog(scope);
    },
  };
}
