// The two WS-T screens of W-06: the catalog home of a desktop and the project's sessions list.
import { Link } from "@tanstack/react-router";
import { useState } from "react";
import { useCatalog } from "@/data";
import { Button } from "@/ui/button";
import { Icon } from "@/ui/Icon";
import { Input } from "@/ui/input";
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
import { MonoHome } from "@/features/monos/MonoHome";
import { useMonoPrefs } from "@/features/monos/monoPrefs";
import { WorkspaceFrame, useReadOnly } from "./WorkspaceFrame";
import { SEARCH_DEBOUNCE_MS, useDebouncedValue } from "./useSessionPages";

/** F-14 empty state: nothing is shared with this device yet. */
export function WorkspaceEmpty({ desktopId }: { desktopId: string }) {
  return (
    <section className="flex min-h-[50dvh] flex-col items-start justify-center gap-5 py-8" data-testid="workspace-empty">
      <h2 className="text-display font-semibold">Make yourself at home.</h2>
      <p className="text-body text-muted-text">Choose the projects and Monos you want on this device.</p>
      <Button asChild size="lg">
        <Link to="/d/$desktopId/settings" params={{ desktopId }}>
          Choose projects
        </Link>
      </Button>
    </section>
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

  return (
    // "route-stub" is kept as the outlet marker used by link/LinkProvider.test.tsx and version/layout.test.tsx (WS-L); rename when their owner switches to a real marker.
    <WorkspaceFrame desktopId={desktopId} title="Workspace" testId="route-stub">
      {monosOnly ? (
        <MonoHome desktopId={desktopId} />
      ) : !data ? (
        catalog.isError ? (
          <Notice variant="error">Could not load your workspace.</Notice>
        ) : (
          <p role="status" className="text-label text-muted-text">Loading…</p>
        )
      ) : empty ? (
        <WorkspaceEmpty desktopId={desktopId} />
      ) : (
        <div className="flex flex-col gap-6">
          {data.projects.length > 0 ? (
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

/** `/d/:desktopId/p/:projectId`: the project's sessions list with search and the now-working pill. */
export function ProjectScreen({ desktopId, projectId }: { desktopId: string; projectId: string }) {
  const catalog = useCatalog();
  const readOnly = useReadOnly();
  const [text, setText] = useState("");
  const query = useDebouncedValue(text, SEARCH_DEBOUNCE_MS);
  const [filter, updateFilter] = useSessionFilter();
  const [worktree, setWorktree] = useState<Worktree | null>(null);
  const project = catalog.data?.projects.find((candidate) => candidate.id === projectId);
  const missing = !!catalog.data && !project;
  const sessions = useProjectSessions(projectId, query, worktree);

  const newSession = readOnly ? (
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
    <WorkspaceFrame desktopId={desktopId} title={project?.name ?? "Project"} subtitle={project?.path} actions={missing ? null : newSession}>
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
        <>
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
        </>
      )}
    </WorkspaceFrame>
  );
}
