// F-21: the project's sessions, newest first, paged in as the list nears its end, virtualized. The sessions
// are read by `useProjectSessions` (the screen needs them for the filter menu), narrowed by the worktree
// (F-22) and the filter (F-23), and shown here. The desktop's session folders come first, as in its sidebar, each with its
// sessions (a folder folds on this device); the sessions in no folder follow, virtualized.
import { useEffect } from "react";
import { useCatalog } from "@/data";
import { Notice } from "@/ui/Notice";
import { FolderHeader } from "./FolderHeader";
import { folderedIds, groupSessions, matchesQuery, projectFolders, useFolderCollapse } from "./folders";
import { SESSION_ROW_HEIGHT, SessionCard } from "./SessionCard";
import { applySessionFilter, inWorktree, isFilterOn, type SessionListFilter, type Worktree } from "./sessionListFilter";
import { useSessionPages, visibleSessions } from "./useSessionPages";
import { useWindowedList } from "./useWindowedList";

/** The project's sessions for a search, scoped to a worktree; the filter is applied by the list. */
export function useProjectSessions(projectId: string, query: string, worktree: Worktree | null) {
  const needle = query.trim();
  const catalog = useCatalog();
  const pages = useSessionPages(projectId, needle);
  const recent = (catalog.data?.sessions ?? []).filter((session) => session.projectId === projectId);
  const sessions = visibleSessions(pages.paged, recent, needle).filter((session) => inWorktree(session, worktree));
  const folders = projectFolders(catalog.data?.folders, projectId);
  // Every session the client has, the catalog's copy first: a folder's members may be older than the loaded pages.
  const known = new Map([...(pages.paged ?? []), ...recent].map((session) => [session.id, session] as const));
  return { needle, catalog, pages, sessions, folders, known };
}

export type ProjectSessions = ReturnType<typeof useProjectSessions>;

export interface SessionsListProps {
  desktopId: string;
  data: ProjectSessions;
  filter: SessionListFilter;
  worktree: Worktree | null;
  readOnly: boolean;
}

export function SessionsList({ desktopId, data, filter, worktree, readOnly }: SessionsListProps) {
  const { needle, catalog, pages } = data;
  const collapse = useFolderCollapse(desktopId, catalog.data?.folders);
  const inFolder = folderedIds(data.folders);
  const groups = groupSessions(
    data.folders,
    data.known,
    (session) => matchesQuery(session, needle) && inWorktree(session, worktree) && applySessionFilter([session], filter).length > 0,
  );
  const sessions = applySessionFilter(
    data.sessions.filter((session) => !inFolder.has(session.id)),
    filter,
  );
  // Folder sections sit above the virtualized list: when they change height, it measures again.
  const layoutKey = groups.map(({ folder, sessions: members }) => `${folder.id}:${!!needle || !collapse.collapsed(folder) ? members.length : 0}`).join("|");
  const [listRef, list] = useWindowedList(sessions.length, SESSION_ROW_HEIGHT, undefined, layoutKey);

  // The next page is asked for when the window reaches the last rows.
  // After a failed page it waits for the Retry button instead of asking again (no request loop).
  const { loadMore, nextError, loadingMore } = pages;
  const nearEnd = pages.hasMore && !nextError && list.end >= sessions.length - 5;
  useEffect(() => {
    if (nearEnd) loadMore();
  }, [nearEnd, loadMore, sessions.length, loadingMore]);

  if (sessions.length === 0 && groups.length === 0) {
    if (pages.loading && !pages.error) return <p role="status" className="px-3.5 pt-2 text-label text-muted-text">Loading sessions…</p>;
    if (pages.error && !catalog.data) return <Notice variant="error">Could not load the sessions.</Notice>;
    if (pages.nextError) return <MoreError onRetry={pages.retryMore} />;
    if (pages.hasMore) return <p role="status" className="px-3.5 pt-2 text-label text-muted-text">Loading sessions…</p>;
    const narrowed = isFilterOn(filter) || !!worktree;
    return <p className="px-3.5 pt-2 text-label text-muted-text">{needle ? "No matching sessions." : narrowed ? "No sessions match the filter." : "No sessions yet."}</p>;
  }

  return (
    <div data-testid="sessions-list">
      {groups.map(({ folder, sessions: members }) => {
        // A search opens every folder, as the desktop's does; otherwise this device's own toggle (or the desktop's) rules.
        const open = !!needle || !collapse.collapsed(folder);
        const listId = `session-folder-${folder.id}`;
        return (
          <section key={folder.id} aria-label={`Folder ${folder.name}`} data-testid="session-folder" className="pb-1">
            <FolderHeader desktopId={desktopId} folder={folder} count={members.length} open={open} onToggle={() => collapse.toggle(folder)} readOnly={readOnly} controls={listId} />
            <ul id={listId} aria-label={`Sessions in ${folder.name}`} hidden={!open}>
              {open
                ? members.map((session) => (
                    <li key={session.id} className="pl-3">
                      <SessionCard desktopId={desktopId} session={session} readOnly={readOnly} />
                    </li>
                  ))
                : null}
            </ul>
          </section>
        );
      })}
      <ul ref={listRef} aria-label="Sessions" style={{ paddingTop: list.before, paddingBottom: list.after }}>
        {sessions.slice(list.start, list.end).map((session) => (
          <li key={session.id}>
            <SessionCard desktopId={desktopId} session={session} readOnly={readOnly} />
          </li>
        ))}
      </ul>
      {pages.loadingMore ? <p role="status" className="px-3.5 py-3 text-meta text-muted-text">Loading more…</p> : null}
      {pages.nextError ? <MoreError onRetry={pages.retryMore} /> : null}
    </div>
  );
}

function MoreError({ onRetry }: { onRetry: () => void }) {
  return (
    <p role="alert" className="px-3.5 py-3 text-meta text-muted-text">
      Could not load more sessions.{" "}
      <button type="button" onClick={onRetry} className="font-semibold text-content underline">
        Retry
      </button>
    </p>
  );
}
