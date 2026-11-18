// F-21: a project's sessions a page at a time, newest first (`sessions.list`, limit 30), cached per
// `projectId + query`. Ported from mobile/src/features/sessions/useSessionPages.ts onto TanStack Query.
import { useInfiniteQuery } from "@tanstack/react-query";
import type { SessionCard } from "@monoptah/protocol";
import { LINK_QUERY_DEFAULTS } from "@/data";
import { useLinkConnection, useLinkDesktopId } from "@/link";
import { useEffect, useState } from "react";

export const SESSION_PAGE = 30;
/** Typing waits for a pause before it asks the desktop. */
export const SEARCH_DEBOUNCE_MS = 250;

/** `value`, following changes after `delay` ms; the first value is immediate. */
export function useDebouncedValue<T>(value: T, delay: number): T {
  const [debounced, setDebounced] = useState(value);
  useEffect(() => {
    if (Object.is(debounced, value)) return;
    const timer = setTimeout(() => setDebounced(value), delay);
    return () => clearTimeout(timer);
  }, [value, delay, debounced]);
  return debounced;
}

/** Appends pages, skipping ids already listed (new sessions shift the offsets). */
export function dedupeSessions(pages: ReadonlyArray<{ sessions: readonly SessionCard[] }>): SessionCard[] {
  const seen = new Set<string>();
  const out: SessionCard[] = [];
  for (const page of pages) {
    for (const session of page.sessions) {
      if (seen.has(session.id)) continue;
      seen.add(session.id);
      out.push(session);
    }
  }
  return out;
}

/**
 * What the list shows: the paged sessions, each replaced by the catalog's fresher copy when it has one,
 * or, until the first page lands, the catalog's recent sessions matching the search.
 */
export function visibleSessions(paged: readonly SessionCard[] | null, recent: readonly SessionCard[], query: string): SessionCard[] {
  if (paged) {
    const live = new Map(recent.map((session) => [session.id, session]));
    return paged.map((session) => live.get(session.id) ?? session);
  }
  const needle = query.toLowerCase();
  return recent.filter((s) => !needle || [s.title, s.model, s.branch].some((field) => field.toLowerCase().includes(needle)));
}

export function useSessionPages(projectId: string, query: string, enabled = true) {
  const desktopId = useLinkDesktopId();
  const connection = useLinkConnection();
  const result = useInfiniteQuery({
    ...LINK_QUERY_DEFAULTS,
    // Part of the desktop's key space, so a revoked link drops it with the rest.
    queryKey: ["link", desktopId, "sessions.list", { projectId, query, paged: true }],
    initialPageParam: 0,
    queryFn: ({ pageParam, signal }) => connection.request("sessions.list", { projectId, query, offset: pageParam, limit: SESSION_PAGE }, { signal }),
    getNextPageParam: (last, pages) => (last.hasMore ? pages.reduce((count, page) => count + page.sessions.length, 0) : undefined),
    // Opening a list reads its first page again; the catalog poll keeps the rows fresh after that.
    staleTime: 0,
    enabled,
  });
  const paged = result.data ? dedupeSessions(result.data.pages) : null;
  return {
    paged,
    hasMore: result.hasNextPage,
    loadMore: () => {
      if (result.hasNextPage && !result.isFetchingNextPage) void result.fetchNextPage();
    },
    loadingMore: result.isFetchingNextPage,
    loading: result.isPending,
    error: result.isError,
  };
}
