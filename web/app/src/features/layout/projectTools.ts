// The project tools the shell offers for the project of the current screen: the Files, Changes and Git panels and
// the background picker. One hook feeds every entry point (the project bar and the command palette), so they
// show and hide the same things. Each panel is offered only when its kind is registered and the
// session-capabilities predicate allows it for the session's target and the desktop's capabilities.
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

export interface ProjectTools {
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
  const context: SessionCapabilityContext = { target: scopeTarget, desktopCapabilities: capabilities };
  const panels = scope ? PANEL_KINDS.filter((kind) => hasPanel(kind) && sessionCan(kind, context)) : [];
  return {
    scope,
    panels,
    isOpen: (kind) => panelMatchesScope(open, scope) && open.kind === kind,
    toggle: (kind) => {
      if (scope) togglePanel(kind, scope, context);
    },
    open: (kind) => (scope ? openPanel(kind, scope, context) : false),
    openBackground: () => {
      if (scope) openBackgroundDialog(scope);
    },
  };
}
