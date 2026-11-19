// The session-list sidebar of the classic layout (design-system.md section 11): from 1024 px the app shell puts it
// between the workspace sidebar and the chat, for the project of the current screen (a project route, or the session's
// project). It mirrors the native classic layout's second sidebar: project title and path, the branch picker, search,
// filter and the sessions, with a "New session" button. The chat pane takes the rest of the width.
import { Link } from "@tanstack/react-router";
import { useCatalog } from "@/data";
import { Button } from "@/ui/button";
import { Icon } from "@/ui/Icon";
import { Notice } from "@/ui/Notice";
import { SessionListBody } from "./SessionListBody";
import { useActivityTracking, useReadOnly } from "./WorkspaceFrame";

export interface SessionListPaneProps {
  desktopId: string;
  projectId: string;
  /** Pixel width, set by the shell (resizable, persisted). */
  width: number;
  /**
   * `h1` on the project routes, where the chat pane (the New session composer) has no title of its own; `h2` where the
   * chat shows its own h1 (a session, a project tool page).
   */
  heading: "h1" | "h2";
  /** The chat already shows the New session page (the project URL and `/new`): no button that points at the page already open. */
  newSessionOpen?: boolean;
}

export function SessionListPane({ desktopId, projectId, width, heading, newSessionOpen = false }: SessionListPaneProps) {
  const catalog = useCatalog();
  const readOnly = useReadOnly();
  // The classic layout shows no WorkspaceFrame on project and session routes, so the list that holds the now-working
  // pill feeds the activity tracker (it is mounted from 1024px only, where the frame is not: no double tracking).
  useActivityTracking(desktopId);
  const project = catalog.data?.projects.find((candidate) => candidate.id === projectId);
  const missing = !!catalog.data && !project;
  const Heading = heading;
  return (
    <section aria-label="Project sessions" data-testid="session-list-pane" style={{ width }} className="flex h-full min-h-0 shrink-0 flex-col bg-background">
      <header className="flex shrink-0 items-start gap-2 px-4 pt-3 pb-3">
        <div className="min-w-0 flex-1">
          <Heading className="wrap-anywhere text-title font-semibold">{project?.name ?? "Project"}</Heading>
          {project?.path ? (
            // A path loses its start, not the folder that tells projects apart: the ellipsis goes on the left.
            <p className="truncate text-left text-small text-muted-text" dir="rtl" title={project.path}>
              <bdi>{project.path}</bdi>
            </p>
          ) : null}
        </div>
        {missing || newSessionOpen ? null : readOnly ? (
          <Button type="button" size="circle" variant="secondary" disabled aria-label="New session" title="This device is read-only">
            <Icon name="compose" size={18} />
          </Button>
        ) : (
          <Button asChild size="circle" variant="secondary">
            <Link to="/d/$desktopId/p/$projectId/new" params={{ desktopId, projectId }} aria-label="New session" title="New session">
              <Icon name="compose" size={18} />
            </Link>
          </Button>
        )}
      </header>
      <div className="min-h-0 flex-1 overflow-y-auto px-4 pb-4">
        {missing ? (
          <section className="flex flex-col items-start gap-3 py-4">
            <Notice variant="error" className="w-full">
              This project is not on this device
            </Notice>
            <Button asChild variant="outline" size="sm">
              <Link to="/d/$desktopId" params={{ desktopId }}>
                Back to the workspace
              </Link>
            </Button>
          </section>
        ) : (
          <SessionListBody desktopId={desktopId} projectId={projectId} />
        )}
      </div>
    </section>
  );
}
