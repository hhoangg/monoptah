// The two WS-T screens of W-06: the catalog home of a desktop and the project's sessions list.
import { Link } from "@tanstack/react-router";
import { useCallback, useState } from "react";
import { createPortal } from "react-dom";
import { useCatalog } from "@/data";
import { ProjectBar } from "@/features/layout/ProjectBar";
import { Button } from "@/ui/button";
import { DelayedSpinner } from "@/ui/DelayedSpinner";
import { EmptyState } from "@/ui/EmptyState";
import { Icon } from "@/ui/Icon";
import { Input } from "@/ui/input";
import { useMediaQuery } from "@/ui/useMediaQuery";
import { Notice } from "@/ui/Notice";
import { NowWorking } from "./NowWorking";
import { SessionCard } from "./SessionCard";
import { SessionFilterMenu } from "./SessionFilterMenu";
import { SessionsList, useProjectSessions } from "./SessionsList";
import { useCatalogUnreadMonos } from "./monoSeen";
import { harnessesInSessions } from "./sessionFilters";
import { useSessionFilter, type Worktree } from "./sessionListFilter";
import { WorkspaceMenu } from "./WorkspaceMenu";
import { MonoRow, ProjectRow } from "./WorkspaceNav";
import { NewSessionScreen } from "./NewSessionScreen";
import { MonoHome } from "@/features/monos/MonoHome";
import { useMonoPrefs } from "@/features/monos/monoPrefs";
import { WIDE_QUERY, WorkspaceFrame, useReadOnly } from "./WorkspaceFrame";
import { SEARCH_DEBOUNCE_MS, useDebouncedValue } from "./useSessionPages";

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
          <Link to="/d/$desktopId/settings" params={{ desktopId }}>
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
    <WorkspaceFrame desktopId={desktopId} title="Workspace" testId="route-stub" narrow>
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
                <Link to="/desktops">All desktops</Link>
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

/** F-18: from 1024 px (design-system.md section 11, `--breakpoint-lg`) the sessions list and the New session page share the screen. */
export const SPLIT_QUERY = "(min-width: 64rem)";

/**
 * The two project routes, `/p/:projectId` (sessions) and `/p/:projectId/new` (New session). Below 1024 px each is
 * its own page; from 1024 px both show side by side (list left, New session right) on either URL.
 *
 * The New session page is rendered once, while visible, through a portal into a host element that is moved between the
 * full-page slot and the split pane. React sees the same element in the same place on both sides of 1024 px, so
 * a rotation or a window resize keeps the typed draft, the attachments and the chosen model.
 */
export function ProjectRoute({ desktopId, projectId, page }: { desktopId: string; projectId: string; page: "sessions" | "new" }) {
  const split = useMediaQuery(SPLIT_QUERY);
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
  return (
    <>
      {page === "new" && !split ? null : <ProjectScreen desktopId={desktopId} projectId={projectId} split={split} detailRef={place} />}
      {split ? null : page === "new" ? <div ref={place} className="flex min-h-0 flex-1 flex-col" /> : null}
      {/* Mounted only while it can be seen: a phone on the sessions page must not fetch models for it. The draft survives in the per-project store (newSessionDraft.ts). */}
      {split || page === "new" ? createPortal(<NewSessionScreen desktopId={desktopId} projectId={projectId} embedded={split} />, host) : null}
    </>
  );
}

/** `/d/:desktopId/p/:projectId`: the project's sessions list with search and the now-working pill. */
export function ProjectScreen({
  desktopId,
  projectId,
  split = false,
  detailRef,
}: {
  desktopId: string;
  projectId: string;
  split?: boolean;
  /** Where the New session page is placed in the split layout. */
  detailRef?: (element: HTMLElement | null) => void;
}) {
  const catalog = useCatalog();
  const readOnly = useReadOnly();
  const wide = useMediaQuery(WIDE_QUERY);
  const [text, setText] = useState("");
  const query = useDebouncedValue(text, SEARCH_DEBOUNCE_MS);
  const [filter, updateFilter] = useSessionFilter();
  const [worktree, setWorktree] = useState<Worktree | null>(null);
  const project = catalog.data?.projects.find((candidate) => candidate.id === projectId);
  const missing = !!catalog.data && !project;
  const sessions = useProjectSessions(projectId, query, worktree);

  // With the New session page beside the list, the header button would only point at what is already open.
  const newSession = split ? null : readOnly ? (
    <Button type="button" size="pill" disabled title="This device is read-only">
      <Icon name="compose" size={18} />
      New session
    </Button>
  ) : (
    <Button asChild size="pill">
      <Link to="/d/$desktopId/p/$projectId/new" params={{ desktopId, projectId }}>
        <Icon name="compose" size={18} />
        New session
      </Link>
    </Button>
  );

  return (
    <WorkspaceFrame desktopId={desktopId} title={project?.name ?? "Project"} subtitle={project?.path} actions={missing ? null : newSession} fill={split}>
      {wide ? null : (
        <div className="-mx-4 min-[24rem]:-mx-5">
          <ProjectBar variant="bar" />
        </div>
      )}
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
        <div className={split ? "grid min-h-0 flex-1 grid-cols-[minmax(0,1fr)_minmax(0,1fr)] grid-rows-[minmax(0,1fr)] gap-6" : undefined} data-testid={split ? "project-split" : undefined}>
          <div className={split ? "min-h-0 min-w-0 overflow-y-auto pb-6" : "min-w-0"}>
          {project ? (
            <div className="pb-2">
              <WorkspaceMenu
                desktopId={desktopId}
                projectId={projectId}
                projectPath={project.path}
                checkoutBranch={project.branch}
                value={worktree}
                onChange={setWorktree}
                readOnly={readOnly}
              />
            </div>
          ) : null}
          <div className="flex items-center gap-2 pb-3">
            <Input type="search" aria-label="Search sessions" placeholder="Search sessions" autoComplete="off" value={text} onChange={(event) => setText(event.target.value)} />
            <SessionFilterMenu value={filter} onChange={updateFilter} harnesses={harnessesInSessions(sessions.sessions)} />
          </div>
          <SessionsList desktopId={desktopId} data={sessions} filter={filter} worktree={worktree} readOnly={readOnly} />
          <NowWorking desktopId={desktopId} projectId={projectId} readOnly={readOnly} />
          </div>
          {split ? (
            <section ref={detailRef} aria-label="New session" data-testid="project-split-detail" className="flex h-full min-h-0 min-w-0 flex-col" />
          ) : null}
        </div>
      )}
    </WorkspaceFrame>
  );
}
