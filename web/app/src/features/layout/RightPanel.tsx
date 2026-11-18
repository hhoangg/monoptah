// The right-panel host (design-system.md section 11): a vaul sheet (92dvh) below 640px, a 360px overlay
// that slides over the pane at 640-1024px, a docked resizable panel from 1024px that stays beside the
// main pane. What is inside comes from the registry in panels.ts; Esc is handled in shortcuts.ts.
import { Drawer, DrawerContent, DrawerTitle } from "@/ui/drawer";
import { Icon } from "@/ui/Icon";
import { PANEL_WIDTH, TABLET_PANEL_WIDTH, useLayoutPrefs } from "./layoutPrefs";
import { closePanel, getPanel, usePanels } from "./panels";
import { Splitter } from "./Splitter";

export type PanelMode = "sheet" | "overlay" | "docked";

/** The sticky offset the WS-T sidebar uses below the 2.75rem bar. */
const STICKY = "sticky top-[calc(2.75rem+env(safe-area-inset-top))] h-[calc(100dvh-2.75rem-env(safe-area-inset-top))]";

function PanelBody({ desktopId }: { desktopId: string }) {
  const open = usePanels((state) => state.open);
  const definition = open && open.desktopId === desktopId ? getPanel(open.kind) : undefined;
  if (!open || !definition) return null;
  const Content = definition.component;
  return (
    <div data-testid="right-panel" data-kind={open.kind} className="@container flex h-full min-h-0 flex-col">
      <div className="flex min-h-14 shrink-0 items-center gap-2 border-b border-stroke pr-2 pl-4">
        <h2 className="min-w-0 flex-1 truncate text-title font-semibold">{definition.title}</h2>
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

export function RightPanel({ mode, desktopId }: { mode: PanelMode; desktopId: string }) {
  const open = usePanels((state) => state.open);
  const panelWidth = useLayoutPrefs((state) => state.panelWidth);
  const setPanelWidth = useLayoutPrefs((state) => state.setPanelWidth);
  const definition = open && open.desktopId === desktopId ? getPanel(open.kind) : undefined;
  const visible = !!open && !!definition;

  if (mode === "sheet") {
    return (
      <Drawer open={visible} onOpenChange={(next) => (next ? undefined : closePanel())}>
        <DrawerContent aria-describedby={undefined} className="h-[92dvh]">
          {definition ? <DrawerTitle className="sr-only">{definition.title}</DrawerTitle> : null}
          <div className="min-h-0 flex-1">
            <PanelBody desktopId={desktopId} />
          </div>
        </DrawerContent>
      </Drawer>
    );
  }
  if (!visible) return null;
  if (mode === "overlay") {
    return (
      <aside
        aria-label={definition.title}
        data-testid="panel-overlay"
        style={{ width: TABLET_PANEL_WIDTH }}
        className="fixed inset-y-0 right-0 z-40 max-w-full border-l border-stroke bg-sheet pt-[env(safe-area-inset-top)] pb-[env(safe-area-inset-bottom)] text-content transition-transform duration-200 starting:translate-x-full motion-reduce:transition-none"
      >
        <PanelBody desktopId={desktopId} />
      </aside>
    );
  }
  return (
    <>
      <Splitter label="Resize panel" value={panelWidth} min={PANEL_WIDTH.min} max={PANEL_WIDTH.max} onChange={setPanelWidth} grow={-1} />
      <aside aria-label={definition.title} data-testid="panel-docked" style={{ width: panelWidth }} className={`${STICKY} shrink-0 border-l border-stroke bg-sheet`}>
        <PanelBody desktopId={desktopId} />
      </aside>
    </>
  );
}
