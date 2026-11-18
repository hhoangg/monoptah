// The `app-shell` slot: one shared layout for every /d/:desktopId/* route (design-system.md section 11).
//   < 640px      passthrough: screens are single column, the WS-T drawer is the navigation. Right panels
//                open as a sheet.
//   640-1024px   persistent 300px sidebar, collapsible to a rail (monoptah.layout.sidebarCollapsed).
//                Right panels slide over as a 360px overlay.
//   >= 1024px    persistent 320-360px sidebar with a splitter (monoptah.layout.sidebarWidth). Right panels
//                dock beside the main pane, 380-480px with a splitter (monoptah.layout.panelWidth).
// The shell also mounts the global shortcuts (shortcuts.ts) and the command palette (F-15).
import { useParams, useRouterState } from "@tanstack/react-router";
import type { ReactNode } from "react";
import { WorkspaceNav } from "@/features/sessions/WorkspaceNav";
import { Icon } from "@/ui/Icon";
import { useMediaQuery } from "@/ui/useMediaQuery";
import { CommandPalette } from "./CommandPalette";
import { SIDEBAR_WIDTH, TABLET_SIDEBAR_WIDTH, useLayoutPrefs } from "./layoutPrefs";
import { openCommandPalette } from "./palette";
import { RightPanel, type PanelMode } from "./RightPanel";
import { routeHasOwnSidebar } from "./shellRoutes";
import { useGlobalShortcuts } from "./shortcuts";
import { Splitter } from "./Splitter";

const TABLET_QUERY = "(min-width: 640px)";
const DESKTOP_QUERY = "(min-width: 1024px)";

/** The sticky offset the WS-T sidebar uses below the 2.75rem bar. */
const STICKY = "sticky top-[calc(2.75rem+env(safe-area-inset-top))] h-[calc(100dvh-2.75rem-env(safe-area-inset-top))]";

const barButton =
  "inline-flex size-11 shrink-0 items-center justify-center rounded-pill bg-fill text-content hover:bg-selection pointer-fine:size-9";

function SearchButton({ compact }: { compact?: boolean }) {
  return compact ? (
    <button type="button" aria-label="Search" aria-keyshortcuts="Control+K Meta+K" onClick={openCommandPalette} className={barButton}>
      <Icon name="search" size={20} />
    </button>
  ) : (
    <button
      type="button"
      aria-label="Search"
      aria-keyshortcuts="Control+K Meta+K"
      onClick={openCommandPalette}
      className="flex min-h-11 min-w-0 flex-1 items-center gap-2.5 rounded-row bg-fill px-3 text-label text-muted-text hover:bg-selection pointer-fine:min-h-9"
    >
      <Icon name="search" size={18} />
      <span className="flex-1 text-left">Search</span>
      <kbd className="font-sans text-meta text-faint">Ctrl K</kbd>
    </button>
  );
}

function Sidebar({ desktopId, tablet }: { desktopId: string; tablet: boolean }) {
  const collapsed = useLayoutPrefs((state) => state.sidebarCollapsed);
  const setCollapsed = useLayoutPrefs((state) => state.setSidebarCollapsed);
  const width = useLayoutPrefs((state) => state.sidebarWidth);
  const setWidth = useLayoutPrefs((state) => state.setSidebarWidth);

  if (tablet && collapsed) {
    return (
      <aside data-testid="app-sidebar" data-state="collapsed" className={`${STICKY} flex w-16 shrink-0 flex-col items-center gap-2 border-r border-stroke bg-drawer py-3`}>
        <button type="button" aria-label="Expand sidebar" aria-expanded="false" onClick={() => setCollapsed(false)} className={barButton}>
          <Icon name="sidebar" size={20} />
        </button>
        <SearchButton compact />
      </aside>
    );
  }
  return (
    <>
      <aside
        data-testid="app-sidebar"
        data-state="expanded"
        style={{ width: tablet ? TABLET_SIDEBAR_WIDTH : width }}
        className={`${STICKY} flex shrink-0 flex-col border-r border-stroke bg-drawer`}
      >
        <div className="flex items-center gap-2 px-4 pt-3">
          <SearchButton />
          {tablet ? (
            <button type="button" aria-label="Collapse sidebar" aria-expanded="true" onClick={() => setCollapsed(true)} className={barButton}>
              <Icon name="sidebar" size={20} />
            </button>
          ) : null}
        </div>
        <div className="min-h-0 flex-1">
          <WorkspaceNav desktopId={desktopId} />
        </div>
      </aside>
      {tablet ? null : <Splitter label="Resize sidebar" value={width} min={SIDEBAR_WIDTH.min} max={SIDEBAR_WIDTH.max} onChange={setWidth} grow={1} />}
    </>
  );
}

export function AppShell({ children }: { children?: ReactNode }) {
  const { desktopId } = useParams({ strict: false }) as { desktopId?: string };
  const pathname = useRouterState({ select: (state) => state.location.pathname });
  const wide = useMediaQuery(TABLET_QUERY);
  const desktop = useMediaQuery(DESKTOP_QUERY);
  useGlobalShortcuts();

  if (!desktopId) return <>{children}</>;
  const mode: PanelMode = !wide ? "sheet" : desktop ? "docked" : "overlay";
  const sidebar = wide && !routeHasOwnSidebar(pathname);

  return (
    <>
      <div data-testid="app-shell" data-layout={!wide ? "mobile" : desktop ? "desktop" : "tablet"} className="flex w-full">
        {sidebar ? <Sidebar desktopId={desktopId} tablet={!desktop} /> : null}
        <div className="@container min-w-0 flex-1">{children}</div>
        {mode === "docked" ? <RightPanel mode="docked" desktopId={desktopId} /> : null}
      </div>
      {mode === "docked" ? null : <RightPanel mode={mode} desktopId={desktopId} />}
      <CommandPalette desktopId={desktopId} />
    </>
  );
}
