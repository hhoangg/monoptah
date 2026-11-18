// The right-panel host (design-system.md section 11): a vaul sheet (92dvh) below 640px, a 360px overlay
// that slides over the pane at 640-1024px, a docked resizable panel from 1024px that stays beside the
// main pane. What is inside comes from the registry in panels.ts; Esc is handled in shortcuts.ts.
// A panel belongs to one project: the host shows it only while the screen's `projectId` equals the project
// the panel was opened for. The overlay closes on a pointerdown outside it (no scrim; an element with
// `data-panel-trigger`, such as the button that toggles the panel, is exempt so it can toggle), moves focus
// to its heading on open and returns it to the opener on close.
import { useEffect, useRef, type ReactNode } from "react";
import { Drawer, DrawerContent, DrawerTitle } from "@/ui/drawer";
import { Icon } from "@/ui/Icon";
import { PANEL_WIDTH, TABLET_PANEL_WIDTH, useLayoutPrefs } from "./layoutPrefs";
import { closePanel, getPanel, panelMatchesScope, usePanels, type OpenPanel, type PanelDefinition } from "./panels";
import { Splitter } from "./Splitter";

export type PanelMode = "sheet" | "overlay" | "docked";

/** The sticky offset the WS-T sidebar uses below the 2.75rem bar. */
const STICKY = "sticky top-[calc(2.75rem+env(safe-area-inset-top))] h-[calc(100dvh-2.75rem-env(safe-area-inset-top))]";

/** The panel to show for the screen's project, with its registration; undefined when there is none. */
function useVisiblePanel(desktopId: string, projectId: string | null): { open: OpenPanel; definition: PanelDefinition } | undefined {
  const open = usePanels((state) => state.open);
  if (!projectId || !panelMatchesScope(open, { desktopId, projectId })) return undefined;
  const definition = getPanel(open.kind);
  return definition ? { open, definition } : undefined;
}

function PanelBody({ desktopId, projectId }: { desktopId: string; projectId: string | null }) {
  const visible = useVisiblePanel(desktopId, projectId);
  if (!visible) return null;
  const { open, definition } = visible;
  const Content = definition.component;
  return (
    <div data-testid="right-panel" data-kind={open.kind} className="@container flex h-full min-h-0 flex-col">
      <div className="flex min-h-14 shrink-0 items-center gap-2 border-b border-stroke pr-2 pl-4">
        <h2 tabIndex={-1} data-panel-heading="" className="min-w-0 flex-1 truncate text-title font-semibold outline-none">{definition.title}</h2>
        <button
          type="button"
          aria-label="Close panel"
          onClick={closePanel}
          className="inline-flex size-11 shrink-0 items-center justify-center rounded-pill text-muted-text hover:bg-selection pointer-fine:size-9"
        >
          <Icon name="close" size={20} />
        </button>
      </div>
      <div className="min-h-0 flex-1 overflow-y-auto">
        <Content desktopId={open.desktopId} projectId={open.projectId} onClose={closePanel} />
      </div>
    </div>
  );
}

const OTHER_OVERLAY = '[role="dialog"], [role="alertdialog"], [role="menu"], [role="listbox"], [data-panel-trigger]';

function OverlayPanel({ title, children }: { title: string; children: ReactNode }) {
  const ref = useRef<HTMLElement>(null);

  useEffect(() => {
    const opener = document.activeElement instanceof HTMLElement ? document.activeElement : null;
    ref.current?.querySelector<HTMLElement>("[data-panel-heading]")?.focus();
    return () => {
      if (opener?.isConnected) opener.focus();
    };
  }, []);

  useEffect(() => {
    const onPointerDown = (event: PointerEvent) => {
      const target = event.target;
      if (!(target instanceof Element) || ref.current?.contains(target) || target.closest(OTHER_OVERLAY)) return;
      closePanel();
    };
    document.addEventListener("pointerdown", onPointerDown);
    return () => document.removeEventListener("pointerdown", onPointerDown);
  }, []);

  return (
    <aside
      ref={ref}
      aria-label={title}
      data-testid="panel-overlay"
      style={{ width: TABLET_PANEL_WIDTH }}
      className="fixed inset-y-0 right-0 z-40 max-w-full border-l border-stroke bg-sheet pt-[env(safe-area-inset-top)] pb-[env(safe-area-inset-bottom)] text-content transition-transform duration-200 starting:translate-x-full motion-reduce:transition-none"
    >
      {children}
    </aside>
  );
}

export interface RightPanelProps {
  mode: PanelMode;
  desktopId: string;
  /** The project of the current screen (null on screens without one: nothing shows). */
  projectId: string | null;
  /** Docked mode: the widest the panel may be so the main pane keeps its minimum (see `dockedPanelMax`). */
  maxWidth?: number;
}

export function RightPanel({ mode, desktopId, projectId, maxWidth = PANEL_WIDTH.max }: RightPanelProps) {
  const panelWidth = useLayoutPrefs((state) => state.panelWidth);
  const setPanelWidth = useLayoutPrefs((state) => state.setPanelWidth);
  const persistPanelWidth = useLayoutPrefs((state) => state.persistPanelWidth);
  const visible = useVisiblePanel(desktopId, projectId);
  const definition = visible?.definition;

  if (mode === "sheet") {
    return (
      <Drawer open={!!visible} onOpenChange={(next) => (next ? undefined : closePanel())}>
        <DrawerContent aria-describedby={undefined} className="h-[92dvh]">
          {definition ? <DrawerTitle className="sr-only">{definition.title}</DrawerTitle> : null}
          <div className="min-h-0 flex-1">
            <PanelBody desktopId={desktopId} projectId={projectId} />
          </div>
        </DrawerContent>
      </Drawer>
    );
  }
  if (!definition) return null;
  if (mode === "overlay") {
    return (
      <OverlayPanel title={definition.title}>
        <PanelBody desktopId={desktopId} projectId={projectId} />
      </OverlayPanel>
    );
  }
  const max = Math.max(PANEL_WIDTH.min, Math.min(PANEL_WIDTH.max, maxWidth));
  const width = Math.min(panelWidth, max);
  return (
    <>
      <Splitter label="Resize panel" value={width} min={PANEL_WIDTH.min} max={max} onChange={setPanelWidth} onCommit={persistPanelWidth} grow={-1} />
      <aside aria-label={definition.title} data-testid="panel-docked" style={{ width }} className={`${STICKY} shrink-0 bg-sheet`}>
        <PanelBody desktopId={desktopId} projectId={projectId} />
      </aside>
    </>
  );
}
