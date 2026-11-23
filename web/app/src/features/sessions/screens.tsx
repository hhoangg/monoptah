// The two WS-T screens of W-06: the catalog home of a desktop and the project's sessions list.
import { Link } from "@tanstack/react-router";
import { useCallback, useState } from "react";
import { createPortal } from "react-dom";
import { useCatalog } from "@/data";
import { useLayoutPrefs } from "@/features/layout/layoutPrefs";
import { ProjectBar } from "@/features/layout/ProjectBar";
import { Button } from "@/ui/button";
import { DelayedSpinner } from "@/ui/DelayedSpinner";
import { EmptyState } from "@/ui/EmptyState";
import { Icon } from "@/ui/Icon";
import { useMediaQuery } from "@/ui/useMediaQuery";
import { Notice } from "@/ui/Notice";
import { IconTip } from "@/ui/tooltip";
import { SessionCard } from "./SessionCard";
import { useCatalogUnreadMonos } from "./monoSeen";
import { MonoRow, ProjectRow, accentFor } from "./WorkspaceNav";
import { ProjectMascot } from "./ProjectMascot";
import { NewSessionScreen } from "./NewSessionScreen";
import { SessionListBody } from "./SessionListBody";
import { MonoHome } from "@/features/monos/MonoHome";
import { useMonoPrefs } from "@/features/monos/monoPrefs";
import { WIDE_QUERY, WorkspaceFrame, useReadOnly } from "./WorkspaceFrame";

/** F-14 empty state: nothing is shared with this device yet. */
export function WorkspaceEmpty({ desktopId }: { desktopId: string }) {
  return (
    <EmptyState
      data-testid="workspace-empty"
      icon="folder"
      title="Make yourself at home."
      description="Choose the projects and Monos you want on this device."
      className="min-h-[50dvh] justify-center"
      action={
        <Button asChild size="lg">
          <Link to="/settings/desktops/$id" params={{ id: desktopId }} hash="desktop-projects">
            Choose projects
          </Link>
        </Button>
      }
    />
  );
}

const RECENT_COUNT = 8;

/** `/d/:desktopId`: Monos, projects and the most recent sessions of the desktop. */
export function HomeScreen({ desktopId }: { desktopId: string }) {
  const catalog = useCatalog();
  const readOnly = useReadOnly();
  const unread = useCatalogUnreadMonos(desktopId);
  const data = catalog.data;
  const recent = data ? [...data.sessions].sort((a, b) => b.updatedAt - a.updatedAt).slice(0, RECENT_COUNT) : [];
  const empty = !!data && data.projects.length === 0 && data.monos.length === 0;
  // F-13: with "Monos only" on, the home is the messaging-style Mono list instead of projects and sessions.
  const monosOnly = useMonoPrefs((state) => state.monosOnly);
  // From 640 px the sidebar lists the projects and Monos; the home repeats them only on a phone.
  const wide = useMediaQuery(WIDE_QUERY);

  return (
    // "route-stub" is kept as the outlet marker used by link/LinkProvider.test.tsx and version/layout.test.tsx (WS-L); rename when their owner switches to a real marker.
    <WorkspaceFrame desktopId={desktopId} title="Workspace" testId="route-stub" narrow hideTitle>
      {monosOnly ? (
        <MonoHome desktopId={desktopId} />
      ) : !data ? (
        catalog.isError ? (
          <div className="flex flex-col items-start gap-3">
            <Notice variant="error" className="w-full">
              Could not load your workspace.
            </Notice>
            <div className="flex flex-wrap items-center gap-2">
              <Button type="button" variant="outline" size="sm" onClick={() => void catalog.refetch()}>
                Try again
              </Button>
              <Button asChild variant="ghost" size="sm">
                <Link to="/settings/desktops">All desktops</Link>
              </Button>
            </div>
          </div>
        ) : (
          <DelayedSpinner label="Loading your workspace" className="m-6" />
        )
      ) : empty ? (
        <WorkspaceEmpty desktopId={desktopId} />
      ) : (
        <div className="flex flex-col gap-6">
          {!wide && data.projects.length > 0 ? (
            <section aria-label="Projects" className="flex flex-col gap-0.5">
              <h2 className="px-2.5 pb-1 text-meta font-medium text-muted-text">Projects</h2>
              {data.projects.map((project) => (
                <ProjectRow key={project.id} desktopId={desktopId} project={project} active={false} />
              ))}
            </section>
          ) : null}
          {data.monos.length > 0 ? (
            <section aria-label="Monos" className="flex flex-col gap-0.5">
              <h2 className="px-2.5 pb-1 text-meta font-medium text-muted-text">Monos</h2>
              {data.monos.map((mono) => (
                <MonoRow key={mono.id} desktopId={desktopId} mono={mono} active={false} unread={unread.has(mono.id)} />
              ))}
            </section>
          ) : null}
          {recent.length > 0 ? (
            <section aria-label="Recent sessions" className="flex flex-col">
              <h2 className="px-2.5 pb-1 text-meta font-medium text-muted-text">Recent sessions</h2>
              {recent.map((session) => (
                <SessionCard key={session.id} desktopId={desktopId} session={session} readOnly={readOnly} />
              ))}
            </section>
          ) : null}
        </div>
      )}
    </WorkspaceFrame>
  );
}

/** F-18: from 1024 px (design-system.md section 11, `--breakpoint-lg`) the app shell shows the project's session list as a sidebar and the New session page fills the chat pane. */
export const SPLIT_QUERY = "(min-width: 64rem)";

/**
 * The two project routes, `/p/:projectId` (sessions) and `/p/:projectId/new` (New session). Below 1024 px each is
 * its own page. From 1024 px the session list is the app shell's second sidebar (`SessionListPane`) and both URLs show
 * the New session page, which fills the chat pane edge to edge.
 *
 * The New session page is rendered once, while visible, through a portal into a host element that is moved between the
 * full-page slot and the chat pane. React sees the same element in the same place on both sides of 1024 px, so
 * a rotation or a window resize keeps the typed draft, the attachments and the chosen model.
 */
export function ProjectRoute({ desktopId, projectId, page }: { desktopId: string; projectId: string; page: "sessions" | "new" }) {
  const wideEnough = useMediaQuery(SPLIT_QUERY);
  const classic = useLayoutPrefs((state) => state.layoutMode) === "classic";
  // The focused layout keeps the project page and New session as separate pages, like a tablet.
  const split = wideEnough && classic;
  // The element of the host that holds focus. React removes the old slot (and the host with it) before the new slot's
  // ref runs, which drops the focus, so it is remembered here and given back once the host is placed again. A blur
  // while the host is still in the document is a real one and clears it.
  const [state] = useState(() => {
    const element = document.createElement("div");
    element.className = "flex min-h-0 flex-1 flex-col";
    const created = { host: element, focused: null as HTMLElement | null };
    element.addEventListener("focusin", (event) => {
      created.focused = event.target instanceof HTMLElement ? event.target : null;
    });
    element.addEventListener("focusout", () => {
      queueMicrotask(() => {
        if (element.isConnected) created.focused = null;
      });
    });
    return created;
  });
  const host = state.host;
  const place = useCallback(
    (element: HTMLElement | null) => {
      if (!element || host.parentElement === element) return;
      element.appendChild(host);
      const target = state.focused;
      if (target && host.contains(target) && (document.activeElement === null || document.activeElement === document.body)) target.focus();
    },
    [host, state],
  );
  const showNew = split || page === "new";
  return (
    <>
      {showNew ? <div ref={place} data-testid={split ? "chat-new-session" : undefined} className="flex min-h-0 flex-1 flex-col" /> : <ProjectScreen desktopId={desktopId} projectId={projectId} />}
      {/* Mounted only while it can be seen: a phone on the sessions page must not fetch models for it. The draft survives in the per-project store (newSessionDraft.ts). */}
      {showNew ? createPortal(<NewSessionScreen desktopId={desktopId} projectId={projectId} embedded={split} />, host) : null}
    </>
  );
}

/** `/d/:desktopId/p/:projectId` below 1024 px: the project's sessions list with search and the now-working pill. */
export function ProjectScreen({ desktopId, projectId }: { desktopId: string; projectId: string }) {
  const catalog = useCatalog();
  const readOnly = useReadOnly();
  const wide = useMediaQuery(WIDE_QUERY);
  const project = catalog.data?.projects.find((candidate) => candidate.id === projectId);
  const missing = !!catalog.data && !project;

  // Icon only: the accessible name and the tooltip (fine pointers) say "New session"; 44 px on touch.
  const newSession = readOnly ? (
    // A disabled button gets no hover: the span carries the tooltip.
    <IconTip label="New session is off: this device is read-only">
      <span className="inline-flex">
        <Button type="button" size="circle" disabled aria-label="New session">
          <Icon name="compose" size={20} />
        </Button>
      </span>
    </IconTip>
  ) : (
    <IconTip label="New session">
      <Button asChild size="circle">
        <Link to="/d/$desktopId/p/$projectId/new" params={{ desktopId, projectId }} aria-label="New session">
          <Icon name="compose" size={20} />
        </Link>
      </Button>
    </IconTip>
  );

  return (
    <WorkspaceFrame
      desktopId={desktopId}
      title={project?.name ?? "Project"}
      subtitle={project?.path}
      leading={project ? <ProjectMascot project={project.path} name={project.mascot} color={accentFor(project.name, project.color)} size={32} active={!!project.busy} className="shrink-0" /> : undefined}
      actions={missing ? null : newSession}
      flush
      compact
    >
      {missing ? (
        <section className="flex flex-col items-start gap-4 py-8">
          <h2 className="text-title font-semibold">This project is not on this device</h2>
          <p className="text-body text-muted-text">It may have been removed from the selection on your desktop.</p>
          <Button asChild variant="outline">
            <Link to="/d/$desktopId" params={{ desktopId }}>
              Back to the workspace
            </Link>
          </Button>
        </section>
      ) : (
        <SessionListBody desktopId={desktopId} projectId={projectId} tools={wide ? null : <ProjectBar variant="inline" />} />
      )}
    </WorkspaceFrame>
  );
}
