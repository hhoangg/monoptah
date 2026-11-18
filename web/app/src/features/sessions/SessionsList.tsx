// F-21: the project's sessions, newest first, paged in as the list nears its end, virtualized. The sessions
// are read by `useProjectSessions` (the screen needs them for the filter menu), narrowed by the worktree
// (F-22) and the filter (F-23), and shown here.
import { useEffect } from "react";
import { useCatalog } from "@/data";
import { Notice } from "@/ui/Notice";
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
  return { needle, catalog, pages, sessions };
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
  const sessions = applySessionFilter(data.sessions, filter);
  const [listRef, list] = useWindowedList(sessions.length, SESSION_ROW_HEIGHT);

  // The next page is asked for when the window reaches the last rows.
  // After a failed page it waits for the Retry button instead of asking again (no request loop).
  const { loadMore, nextError, loadingMore } = pages;
  const nearEnd = pages.hasMore && !nextError && list.end >= sessions.length - 5;
  useEffect(() => {
    if (nearEnd) loadMore();
  }, [nearEnd, loadMore, sessions.length, loadingMore]);

  if (sessions.length === 0) {
    if (pages.loading && !pages.error) return <p role="status" className="px-3.5 pt-2 text-label text-muted-text">Loading sessions…</p>;
    if (pages.error && !catalog.data) return <Notice variant="error">Could not load the sessions.</Notice>;
    if (pages.nextError) return <MoreError onRetry={pages.retryMore} />;
    if (pages.hasMore) return <p role="status" className="px-3.5 pt-2 text-label text-muted-text">Loading sessions…</p>;
    const narrowed = isFilterOn(filter) || !!worktree;
    return <p className="px-3.5 pt-2 text-label text-muted-text">{needle ? "No matching sessions." : narrowed ? "No sessions match the filter." : "No sessions yet."}</p>;
  }

  return (
    <div data-testid="sessions-list">
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
