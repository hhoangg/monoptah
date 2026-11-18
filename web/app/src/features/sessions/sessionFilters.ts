// Copied from src/features/sessions/model/sessionFilters.ts at ca73ac2b
// Only the pure filter functions are carried over. The desktop's persistence helpers are not
// (their keys carried the upstream prefix); any web storage for these filters uses a `monoptah.` key.
import { HARNESS_IDS, type Harness, type SessionCard } from "@monoptah/protocol";

/** The fields of a protocol session card that the filters read. */
export type FilterableSession = Pick<SessionCard, "id" | "harness" | "updatedAt">;

export type SessionTimeFilter = "all" | "today" | "7d" | "30d";

export type SessionStatusFilter = {
  working: boolean;
  needsApproval: boolean;
  done: boolean;
};

export type SessionSidebarFilters = {
  showArchived: boolean;
  /** Narrows a Mono's sessions to launches since deleted. */
  showUnavailable?: boolean;
  hiddenHarnesses: Harness[];
  time: SessionTimeFilter;
  status: SessionStatusFilter;
};

export const DEFAULT_SESSION_STATUS_FILTER: SessionStatusFilter = {
  working: false,
  needsApproval: false,
  done: false,
};

export const DEFAULT_SESSION_SIDEBAR_FILTERS: SessionSidebarFilters = {
  showArchived: false,
  hiddenHarnesses: [],
  time: "all",
  status: DEFAULT_SESSION_STATUS_FILTER,
};

/** The known harnesses present in `rows`, in the canonical harness order. */
export function harnessesInSessions(rows: readonly FilterableSession[]): Harness[] {
  const seen = new Set<string>();
  for (const row of rows) seen.add(row.harness);
  return HARNESS_IDS.filter((harness) => seen.has(harness));
}

export function hasActiveSessionFilters(filters: SessionSidebarFilters): boolean {
  return (
    filters.showArchived ||
    !!filters.showUnavailable ||
    filters.hiddenHarnesses.length > 0 ||
    filters.time !== "all" ||
    filters.status.working ||
    filters.status.needsApproval ||
    filters.status.done
  );
}

export function filterSessionsByHarness<T extends FilterableSession>(
  rows: T[],
  hiddenHarnesses: Iterable<string>,
): T[] {
  const hidden = new Set(hiddenHarnesses);
  if (hidden.size === 0) return rows;
  return rows.filter((row) => !hidden.has(row.harness));
}

export function filterSessionsByTime<T extends FilterableSession>(
  rows: T[],
  time: SessionTimeFilter,
  now: number,
): T[] {
  if (time === "all") return rows;
  const start = timeFilterStart(time, now);
  return rows.filter((row) => row.updatedAt >= start);
}

export function filterSessionsByStatus<T extends FilterableSession>(
  rows: T[],
  status: SessionStatusFilter,
  busyIds: Set<string>,
  approvalIds: Set<string>,
  doneIds: Set<string>,
): T[] {
  const any = status.working || status.needsApproval || status.done;
  if (!any) return rows;
  return rows.filter((row) => {
    if (status.working && busyIds.has(row.id)) return true;
    if (status.needsApproval && approvalIds.has(row.id)) return true;
    if (status.done && doneIds.has(row.id)) return true;
    return false;
  });
}

export function timeFilterStart(time: SessionTimeFilter, now: number): number {
  if (time === "today") {
    const date = new Date(now);
    date.setHours(0, 0, 0, 0);
    return date.getTime();
  }
  const dayMs = 24 * 60 * 60 * 1000;
  if (time === "7d") return now - 7 * dayMs;
  if (time === "30d") return now - 30 * dayMs;
  return 0;
}
