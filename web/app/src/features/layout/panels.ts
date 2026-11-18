// Right panels (files, changes, git) of the app shell (design-system.md section 11).
//
// Contents are registered by later tickets (U-05 files, U-06 changes and git); the shell only hosts them:
// a vaul sheet below 640px, a 360px overlay at 640-1024px, a docked resizable panel from 1024px.
// One panel is open at a time. A kind with no registration cannot be opened and offers no entry.
//
// Public API:
//   PANEL_KINDS = ["files", "changes", "git"]; type PanelKind
//   interface PanelTarget { desktopId: string; projectId: string }
//   interface PanelProps extends PanelTarget { onClose(): void }       props of a panel component
//   registerPanel(kind, { title, component }): void   once per kind at startup, a second call throws
//   getPanel(kind) / hasPanel(kind) / usePanelRegistered(kind)         look a registration up
//   openPanel(kind, target, context): boolean   false when the kind is unregistered or the session-capabilities
//                                               `context` (required, so callers cannot skip the check)
//                                               refuses it (the kinds are capability names, so non-local
//                                               sessions hide them)
//   closePanel(): void, togglePanel(kind, target, context): void, closeTopmostPanel(): boolean
//   usePanels: zustand store `{ open, scope }`; `open` is the last opened panel (a PanelTarget and a kind),
//     `scope` the project the shell currently shows (setPanelScope, set by AppShell). A panel is visible
//     only while its target equals the scope, and Esc closes only a visible one. clearPanels() for tests
import type { ComponentType } from "react";
import { create } from "zustand";
import { sessionCan, type SessionCapabilityContext } from "@/ui/hooks";

export const PANEL_KINDS = ["files", "changes", "git"] as const;
export type PanelKind = (typeof PANEL_KINDS)[number];

export interface PanelTarget {
  desktopId: string;
  projectId: string;
}

export interface PanelProps extends PanelTarget {
  onClose: () => void;
}

export interface PanelDefinition {
  title: string;
  component: ComponentType<PanelProps>;
}

const registry = new Map<PanelKind, PanelDefinition>();

export function registerPanel(kind: PanelKind, definition: PanelDefinition): void {
  if (registry.has(kind)) throw new Error(`Panel "${kind}" is already registered`);
  registry.set(kind, definition);
}

export function getPanel(kind: PanelKind): PanelDefinition | undefined {
  return registry.get(kind);
}

export function hasPanel(kind: PanelKind): boolean {
  return registry.has(kind);
}

/** Registrations happen at startup, so this is a plain read; it exists so entry points read like hooks. */
export function usePanelRegistered(kind: PanelKind): boolean {
  return registry.has(kind);
}

/** Drops every registration and closes the open panel. For tests. */
export function clearPanels(): void {
  registry.clear();
  usePanels.setState({ open: null, scope: null });
}

export interface OpenPanel extends PanelTarget {
  kind: PanelKind;
}

export const usePanels = create<{ open: OpenPanel | null; scope: PanelTarget | null }>(() => ({ open: null, scope: null }));

/** The project the shell currently shows (null outside a project or session screen). */
export function setPanelScope(scope: PanelTarget | null): void {
  usePanels.setState({ scope });
}

/** True when `open` belongs to the project in `scope`. */
export function panelMatchesScope(open: OpenPanel | null, scope: PanelTarget | null): open is OpenPanel {
  return !!open && !!scope && open.desktopId === scope.desktopId && open.projectId === scope.projectId;
}

export function openPanel(kind: PanelKind, target: PanelTarget, context: SessionCapabilityContext): boolean {
  if (!registry.has(kind)) return false;
  if (!sessionCan(kind, context)) return false;
  usePanels.setState({ open: { kind, desktopId: target.desktopId, projectId: target.projectId } });
  return true;
}

export function closePanel(): void {
  usePanels.setState({ open: null });
}

export function togglePanel(kind: PanelKind, target: PanelTarget, context: SessionCapabilityContext): void {
  const open = usePanels.getState().open;
  if (open && open.kind === kind && open.desktopId === target.desktopId && open.projectId === target.projectId) closePanel();
  else openPanel(kind, target, context);
}

/** Esc: closes the visible panel (one of the current project). True when there was one. */
export function closeTopmostPanel(): boolean {
  const { open, scope } = usePanels.getState();
  if (!panelMatchesScope(open, scope)) return false;
  closePanel();
  return true;
}
