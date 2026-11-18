// F-21: the project's sessions, newest first, paged in as the list nears its end, virtualized.
import { useEffect } from "react";
import { useCatalog } from "@/data";
import { Notice } from "@/ui/Notice";
import { SESSION_ROW_HEIGHT, SessionCard } from "./SessionCard";
import { useSessionPages, visibleSessions } from "./useSessionPages";
import { useWindowedList } from "./useWindowedList";

export interface SessionsListProps {
  desktopId: string;
  projectId: string;
  /** The debounced search text. */
  query: string;
  readOnly: boolean;
}

export function SessionsList({ desktopId, projectId, query, readOnly }: SessionsListProps) {
  const needle = query.trim();
  const catalog = useCatalog();
  const pages = useSessionPages(projectId, needle);
  const recent = (catalog.data?.sessions ?? []).filter((session) => session.projectId === projectId);
  const sessions = visibleSessions(pages.paged, recent, needle);
  const [listRef, list] = useWindowedList(sessions.length, SESSION_ROW_HEIGHT);

  // The next page is asked for when the window reaches the last rows.
  const { loadMore } = pages;
  const nearEnd = pages.hasMore && list.end >= sessions.length - 5;
  useEffect(() => {
    if (nearEnd) loadMore();
  }, [nearEnd, loadMore, sessions.length]);

  if (sessions.length === 0) {
    if (pages.loading && !pages.error) return <p role="status" className="px-3.5 pt-2 text-label text-muted-text">Loading sessions…</p>;
    if (pages.error && !catalog.data) return <Notice variant="error">Could not load the sessions.</Notice>;
    return <p className="px-3.5 pt-2 text-label text-muted-text">{needle ? "No matching sessions." : "No sessions yet."}</p>;
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
    </div>
  );
}
