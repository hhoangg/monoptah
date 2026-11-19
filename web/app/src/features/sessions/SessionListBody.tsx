// The body of a project's session list: the workspace (branch) menu, search with the filter, the virtualized sessions
// and the now-working pill. Shared by the project page below 1024 px (`ProjectScreen`) and by the session-list sidebar
// of the app shell from 1024 px (`SessionListPane`), so the list is built once.
import { useState } from "react";
import { useCatalog } from "@/data";
import { Input } from "@/ui/input";
import { NowWorking } from "./NowWorking";
import { SessionFilterMenu } from "./SessionFilterMenu";
import { SessionsList, useProjectSessions } from "./SessionsList";
import { harnessesInSessions } from "./sessionFilters";
import { useSessionFilter, type Worktree } from "./sessionListFilter";
import { WorkspaceMenu } from "./WorkspaceMenu";
import { useReadOnly } from "./WorkspaceFrame";
import { SEARCH_DEBOUNCE_MS, useDebouncedValue } from "./useSessionPages";

export function SessionListBody({ desktopId, projectId }: { desktopId: string; projectId: string }) {
  const catalog = useCatalog();
  const readOnly = useReadOnly();
  const [text, setText] = useState("");
  const query = useDebouncedValue(text, SEARCH_DEBOUNCE_MS);
  const [filter, updateFilter] = useSessionFilter();
  const [worktree, setWorktree] = useState<Worktree | null>(null);
  const project = catalog.data?.projects.find((candidate) => candidate.id === projectId);
  const sessions = useProjectSessions(projectId, query, worktree);

  return (
    <div className="flex min-h-full min-w-0 flex-col">
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
  );
}
