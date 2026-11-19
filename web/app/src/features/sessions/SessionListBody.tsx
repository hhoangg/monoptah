// The body of a project's session list: the workspace (branch) menu, search with the filter, the virtualized sessions
// and the now-working pill. Shared by the project page below 1024 px (`ProjectScreen`) and by the session-list sidebar
// of the app shell from 1024 px (`SessionListPane`), so the list is built once.
import { useState, type ReactNode } from "react";
import { useCatalog } from "@/data";
import { Input } from "@/ui/input";
import { NowWorking } from "./NowWorking";
import { useSessionActivity } from "./activityStore";
import { SessionFilterMenu } from "./SessionFilterMenu";
import { SessionsList, useProjectSessions } from "./SessionsList";
import { harnessesInSessions } from "./sessionFilters";
import { useSessionFilter, type Worktree } from "./sessionListFilter";
import { WorkspaceMenu } from "./WorkspaceMenu";
import { useReadOnly } from "./WorkspaceFrame";
import { SEARCH_DEBOUNCE_MS, useDebouncedValue } from "./useSessionPages";

/** `tools`: icon buttons shown on the branch row, at its right end (the project page below 640px). */
export function SessionListBody({ desktopId, projectId, tools = null }: { desktopId: string; projectId: string; tools?: ReactNode }) {
  const catalog = useCatalog();
  const readOnly = useReadOnly();
  const [text, setText] = useState("");
  const query = useDebouncedValue(text, SEARCH_DEBOUNCE_MS);
  const [filter, updateFilter] = useSessionFilter();
  const [worktree, setWorktree] = useState<Worktree | null>(null);
  const project = catalog.data?.projects.find((candidate) => candidate.id === projectId);
  const sessions = useProjectSessions(projectId, query, worktree);
  const activity = useSessionActivity(projectId);
  const hasPill = activity.working.length + activity.finished.length > 0;

  return (
    <div className="flex min-h-full min-w-0 flex-col">
      {project ? (
        <div className="flex items-center justify-between gap-2 pb-2">
          <WorkspaceMenu
            desktopId={desktopId}
            projectId={projectId}
            projectPath={project.path}
            checkoutBranch={project.branch}
            value={worktree}
            onChange={setWorktree}
            readOnly={readOnly}
          />
          {tools}
        </div>
      ) : null}
      <div className="flex items-center gap-2 pb-3">
        <Input type="search" aria-label="Search sessions" placeholder="Search sessions" autoComplete="off" value={text} onChange={(event) => setText(event.target.value)} />
        <SessionFilterMenu value={filter} onChange={updateFilter} harnesses={harnessesInSessions(sessions.sessions)} />
      </div>
      {/* Below 1024px the list ends above the home indicator; with the now-working pill after it, the pill carries the
          inset instead. From 1024px the session-list pane has its own bottom padding. */}
      <div className={hasPill ? "pb-4 min-[64rem]:pb-0" : "pb-[calc(var(--app-bottom-inset)+1rem)] min-[64rem]:pb-0"}>
        <SessionsList desktopId={desktopId} data={sessions} filter={filter} worktree={worktree} readOnly={readOnly} />
      </div>
      <NowWorking desktopId={desktopId} projectId={projectId} readOnly={readOnly} />
    </div>
  );
}
