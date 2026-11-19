// The `app-shell` slot: one shared layout for every /d/:desktopId/* route (design-system.md section 11).
//   < 640px      passthrough: screens are single column, the WS-T drawer is the navigation. Right panels
//                open as a sheet.
//   640-1024px   persistent 300px sidebar, collapsible to a rail (monoptah.layout.sidebarCollapsed).
//                Right panels slide over as a 360px overlay.
//   >= 1024px    persistent 320-360px sidebar with a splitter (monoptah.layout.sidebarWidth). Right panels
//                dock beside the main pane, 380-480px with a splitter (monoptah.layout.panelWidth), never
//                squeezing the main pane below 320px: the panel narrows to fit, and where even 380px does
//                not fit it opens as the overlay instead.
// A right panel belongs to a project: it shows only on that project's screens (`p/$projectId/...` and the
// sessions of that project).
// The shell also mounts the global shortcuts (shortcuts.ts), the command palette (F-15), the project bar (from
// 640px, in the sidebar foot: toggles Files, Changes and Git and opens the background picker) and the background dialog.
import { useParams } from "@tanstack/react-router";
import { useEffect, useSyncExternalStore, type ReactNode } from "react";
import { useCatalog } from "@/data";
import { WorkspaceNav } from "@/features/sessions/WorkspaceNav";
import { Icon } from "@/ui/Icon";
import { IconTip } from "@/ui/tooltip";
import { useMediaQuery } from "@/ui/useMediaQuery";
import { BackgroundDialog } from "./BackgroundDialog";
import { CommandPalette } from "./CommandPalette";
import { SIDEBAR_WIDTH, TABLET_SIDEBAR_WIDTH, dockedPanelMax, useLayoutPrefs } from "./layoutPrefs";
import { openCommandPalette } from "./palette";
import { setPanelScope } from "./panels";
import { ProjectBar } from "./ProjectBar";
import { RightPanel, type PanelMode } from "./RightPanel";
import { isMac, useGlobalShortcuts } from "./shortcuts";
import { Splitter } from "./Splitter";

const TABLET_QUERY = "(min-width: 640px)";
const DESKTOP_QUERY = "(min-width: 1024px)";

/** The sidebar fills the AppShell row, which is a flex child of the fixed-height /d frame (no 100dvh math). */
const FULL = "h-full min-h-0";

const barButton =
  "inline-flex size-11 shrink-0 items-center justify-center rounded-pill bg-fill text-content hover:bg-selection pointer-fine:size-9";

function SearchButton({ compact }: { compact?: boolean }) {
  return compact ? (
    <IconTip label="Search">
      <button type="button" aria-label="Search" aria-keyshortcuts="Control+K Meta+K" onClick={openCommandPalette} className={barButton}>
        <Icon name="search" size={20} />
      </button>
    </IconTip>
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
      <kbd className="font-sans text-meta text-muted-text">{isMac() ? "\u2318K" : "Ctrl K"}</kbd>
    </button>
  );
}

function Sidebar({ desktopId, tablet }: { desktopId: string; tablet: boolean }) {
  const collapsed = useLayoutPrefs((state) => state.sidebarCollapsed);
  const setCollapsed = useLayoutPrefs((state) => state.setSidebarCollapsed);
  const width = useLayoutPrefs((state) => state.sidebarWidth);
  const setWidth = useLayoutPrefs((state) => state.setSidebarWidth);
  const persistWidth = useLayoutPrefs((state) => state.persistSidebarWidth);

  if (tablet && collapsed) {
    return (
      <aside data-testid="app-sidebar" data-state="collapsed" className={`${FULL} flex w-16 shrink-0 flex-col items-center gap-2 border-r border-stroke bg-drawer py-3`}>
        <IconTip label="Expand sidebar">
          <button type="button" aria-label="Expand sidebar" aria-expanded="false" onClick={() => setCollapsed(false)} className={barButton}>
            <Icon name="sidebar" size={20} />
          </button>
        </IconTip>
        <SearchButton compact />
        <div className="mt-auto">
          <ProjectBar variant="rail" />
        </div>
      </aside>
    );
  }
  return (
    <>
      <aside
        data-testid="app-sidebar"
        data-state="expanded"
        style={{ width: tablet ? TABLET_SIDEBAR_WIDTH : width }}
        className={`${FULL} flex shrink-0 flex-col bg-drawer ${tablet ? "border-r border-stroke" : ""}`}
      >
        <div className="flex items-center gap-2 px-4 pt-3">
          <SearchButton />
          {tablet ? (
            <IconTip label="Collapse sidebar">
              <button type="button" aria-label="Collapse sidebar" aria-expanded="true" onClick={() => setCollapsed(true)} className={barButton}>
                <Icon name="sidebar" size={20} />
              </button>
            </IconTip>
          ) : null}
        </div>
        <div className="min-h-0 flex-1">
          <WorkspaceNav desktopId={desktopId} />
        </div>
        <ProjectBar variant="foot" />
      </aside>
      {tablet ? null : <Splitter label="Resize sidebar" value={width} min={SIDEBAR_WIDTH.min} max={SIDEBAR_WIDTH.max} onChange={setWidth} onCommit={persistWidth} grow={1} />}
    </>
  );
}

function subscribeResize(listener: () => void): () => void {
  window.addEventListener("resize", listener);
  return () => window.removeEventListener("resize", listener);
}

/** The window width in px. */
function useViewportWidth(): number {
  return useSyncExternalStore(subscribeResize, () => window.innerWidth, () => 1280);
}

/**
 * The project the current screen belongs to (a project route, or the session's project on a session route) and, on
 * a session route, the session's target kind (what the session-capabilities predicate needs).
 */
function useCurrentScope(): { projectId: string | null; targetKind: string | null } {
  const { projectId, sessionId } = useParams({ strict: false }) as { projectId?: string; sessionId?: string };
  const catalog = useCatalog();
  if (projectId) return { projectId, targetKind: null };
  const session = sessionId ? catalog.data?.sessions.find((candidate) => candidate.id === sessionId) : undefined;
  return { projectId: session?.projectId ?? null, targetKind: session?.target?.kind ?? null };
}

function Shell({ desktopId, children }: { desktopId: string; children?: ReactNode }) {
  const wide = useMediaQuery(TABLET_QUERY);
  const desktop = useMediaQuery(DESKTOP_QUERY);
  const viewport = useViewportWidth();
  const sidebarWidth = useLayoutPrefs((state) => state.sidebarWidth);
  const { projectId, targetKind } = useCurrentScope();
  useGlobalShortcuts();

  useEffect(() => {
    setPanelScope(projectId ? { desktopId, projectId } : null, targetKind ? { kind: targetKind } : null);
    return () => setPanelScope(null);
  }, [desktopId, projectId, targetKind]);

  const dockMax = desktop ? dockedPanelMax(viewport, sidebarWidth) : null;
  const mode: PanelMode = !wide ? "sheet" : dockMax !== null ? "docked" : "overlay";

  return (
    <>
      <div data-testid="app-shell" data-layout={!wide ? "mobile" : desktop ? "desktop" : "tablet"} className="flex min-h-0 w-full flex-1 pr-[env(safe-area-inset-right)] pl-[env(safe-area-inset-left)]">
        {wide ? <Sidebar desktopId={desktopId} tablet={!desktop} /> : null}
        <div className="@container relative flex min-h-0 min-w-0 flex-1 flex-col overflow-y-auto">
          {children}
        </div>
        {mode === "docked" ? <RightPanel mode="docked" desktopId={desktopId} projectId={projectId} maxWidth={dockMax ?? undefined} /> : null}
      </div>
      {mode === "docked" ? null : <RightPanel mode={mode} desktopId={desktopId} projectId={projectId} />}
      <CommandPalette desktopId={desktopId} />
      <BackgroundDialog />
    </>
  );
}

export function AppShell({ children }: { children?: ReactNode }) {
  const { desktopId } = useParams({ strict: false }) as { desktopId?: string };
  if (!desktopId) return <>{children}</>;
  return <Shell desktopId={desktopId}>{children}</Shell>;
}
