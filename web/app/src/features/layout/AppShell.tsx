// The `app-shell` slot: one shared layout for every /d/:desktopId/* route (design-system.md section 11).
//   < 640px      passthrough: screens are single column, the WS-T drawer is the navigation. Right panels
//                open as a sheet.
//   640-1024px   persistent 300px sidebar, collapsible to a rail (monoptah.layout.sidebarCollapsed).
//                Right panels slide over as a 360px overlay.
//   >= 1024px    the classic layout, like the native app: the 320-360px workspace sidebar with a splitter
//                (monoptah.layout.sidebarWidth), then, on the screens of a project, the project's session
//                list as a second sidebar, 240-560px with a splitter (monoptah.layout.sessionListWidth,
//                320 by default), then the chat pane filling the rest edge to edge (ChatPane). Right panels
//                dock beside the chat pane, 380-480px with a splitter (monoptah.layout.panelWidth), never
//                squeezing the chat pane below 320px: the panel narrows to fit, and where even 380px does
//                not fit it opens as the overlay instead. The session list gives way first (it shrinks to
//                keep the chat pane at 320px).
// A right panel belongs to a project: it shows only on that project's screens (`p/$projectId/...` and the
// sessions of that project).
// From 640px an activity rail (ActivityRail.tsx) sits at the far left of the shell: the desktops as avatars, then Cloud,
// Desktop settings and the account menu. It replaces the top bar there (features/desktops/DesktopSwitcher.tsx).
// The shell also mounts the global shortcuts (shortcuts.ts), the command palette (F-15), the project bar (from
// 640px, in the sidebar foot: toggles Files, Changes and Git and opens the background picker) and the background dialog.
import { useParams, useRouterState } from "@tanstack/react-router";
import { useEffect, useSyncExternalStore, type ReactNode } from "react";
import { useCatalog } from "@/data";
import { WorkspaceNav } from "@/features/sessions/WorkspaceNav";
import { Icon } from "@/ui/Icon";
import { IconTip } from "@/ui/tooltip";
import { useMediaQuery } from "@/ui/useMediaQuery";
import { BackgroundDialog } from "./BackgroundDialog";
import { CommandPalette } from "./CommandPalette";
import { SessionListPane } from "@/features/sessions/SessionListPane";
import { ActivityRail } from "./ActivityRail";
import { ChatPane } from "./ChatPane";
import { FooterBar } from "./FooterBar";
import { TerminalDrawer } from "@/features/terminal/TerminalDrawer";
import { PANEL_WIDTH, SESSION_LIST_WIDTH, SIDEBAR_WIDTH, TABLET_SIDEBAR_WIDTH, dockedPanelMax, sessionListMax, useLayoutPrefs } from "./layoutPrefs";
import { openCommandPalette } from "./palette";
import { setPanelScope } from "./panels";
import { ProjectBar } from "./ProjectBar";
import { RightPanel, useVisiblePanel, type PanelMode } from "./RightPanel";
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
          <WorkspaceNav desktopId={desktopId} railed />
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
  const listPref = useLayoutPrefs((state) => state.sessionListWidth);
  const setListWidth = useLayoutPrefs((state) => state.setSessionListWidth);
  const persistListWidth = useLayoutPrefs((state) => state.persistSessionListWidth);
  const panelPref = useLayoutPrefs((state) => state.panelWidth);
  const { projectId, targetKind } = useCurrentScope();
  const visiblePanel = useVisiblePanel(desktopId, projectId);
  // The project routes have no title in the chat pane (the New session page), so the session list names the project (h1).
  const projectRoute = useRouterState({ select: (state) => /\/p\/[^/]+(\/new)?\/?$/.test(state.location.pathname) });
  // On a phone the footer belongs to the workspace screen (the home of the desktop); from 640px it is on every screen.
  const workspaceHome = useRouterState({ select: (state) => state.location.pathname.replace(/\/$/, "") === `/d/${desktopId}` });
  useGlobalShortcuts();

  useEffect(() => {
    setPanelScope(projectId ? { desktopId, projectId } : null, targetKind ? { kind: targetKind } : null);
    return () => setPanelScope(null);
  }, [desktopId, projectId, targetKind]);

  // The session list is the second sidebar of the classic layout: only from 1024 px, only for a project's screens.
  const listShown = desktop && projectId !== null;
  // A docked panel is sized as if the list were at its minimum, then the list shrinks to make room for it.
  const dockMax = desktop ? dockedPanelMax(viewport, sidebarWidth + (listShown ? SESSION_LIST_WIDTH.min : 0)) : null;
  const mode: PanelMode = !wide ? "sheet" : dockMax !== null ? "docked" : "overlay";
  const dockedWidth = mode === "docked" && dockMax !== null && visiblePanel ? Math.min(panelPref, Math.max(PANEL_WIDTH.min, Math.min(PANEL_WIDTH.max, dockMax))) : 0;
  const listMax = sessionListMax(viewport, sidebarWidth + dockedWidth);
  const listWidth = Math.min(listPref, listMax);

  return (
    <>
      <div data-testid="app-shell" data-layout={!wide ? "mobile" : desktop ? "desktop" : "tablet"} className="flex min-h-0 w-full flex-1 pr-[env(safe-area-inset-right)] pl-[env(safe-area-inset-left)]">
        {wide ? (
          <>
            <ActivityRail desktopId={desktopId} />
            <Sidebar desktopId={desktopId} tablet={!desktop} />
          </>
        ) : null}
        {listShown ? (
          <>
            <SessionListPane desktopId={desktopId} projectId={projectId} width={listWidth} heading={projectRoute ? "h1" : "h2"} newSessionOpen={projectRoute} />
            <Splitter label="Resize session list" value={listWidth} min={SESSION_LIST_WIDTH.min} max={listMax} onChange={setListWidth} onCommit={persistListWidth} grow={1} />
          </>
        ) : null}
        <ChatPane desktopId={desktopId}>{children}</ChatPane>
        {mode === "docked" ? <RightPanel mode="docked" desktopId={desktopId} projectId={projectId} maxWidth={dockMax ?? undefined} /> : null}
      </div>
      {wide || workspaceHome ? <FooterBar desktopId={desktopId} tabbed={wide} /> : null}
      {wide ? null : <TerminalDrawer />}
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
