// F-22 and F-23: the project list's filter (working now, hidden agents) and worktree scoping, built on the
// W-21 functions of `sessionFilters.ts`. The filter is kept in localStorage under a `monoptah.` key.
import { useCallback, useState } from "react";
import { HARNESS_IDS, type Harness, type SessionCard } from "@monoptah/protocol";
import { DEFAULT_SESSION_STATUS_FILTER, filterSessionsByHarness, filterSessionsByStatus } from "./sessionFilters";

export const SESSION_FILTER_KEY = "monoptah.sessions.filter";

export interface SessionListFilter {
  /** Only the sessions working now. */
  working: boolean;
  /** Agents left out of the list. */
  hiddenHarnesses: Harness[];
}

export const NO_SESSION_FILTER: SessionListFilter = { working: false, hiddenHarnesses: [] };

export const HARNESS_NAMES: Record<Harness, string> = {
  claude: "Claude",
  codex: "Codex",
  cursor: "Cursor",
  grok: "Grok",
  opencode: "OpenCode",
  pi: "Pi",
  omp: "OMP",
  fx: "FX",
  hermes: "Hermes",
  antigravity: "Antigravity",
  devin: "Devin",
};

export const isFilterOn = (filter: SessionListFilter): boolean => filter.working || filter.hiddenHarnesses.length > 0;

export function readSessionFilter(): SessionListFilter {
  try {
    const raw = window.localStorage.getItem(SESSION_FILTER_KEY);
    const parsed = raw ? (JSON.parse(raw) as { working?: unknown; hiddenHarnesses?: unknown }) : null;
    if (!parsed) return NO_SESSION_FILTER;
    const hidden: unknown[] = Array.isArray(parsed.hiddenHarnesses) ? parsed.hiddenHarnesses : [];
    return {
      working: parsed.working === true,
      hiddenHarnesses: HARNESS_IDS.filter((harness) => hidden.includes(harness)),
    };
  } catch {
    return NO_SESSION_FILTER;
  }
}

export function writeSessionFilter(filter: SessionListFilter): void {
  try {
    window.localStorage.setItem(SESSION_FILTER_KEY, JSON.stringify(filter));
  } catch {
    // Storage blocked or full: the choice lasts until the page reloads.
  }
}

/** The filter, persisted on every change. */
export function useSessionFilter(): [SessionListFilter, (patch: Partial<SessionListFilter>) => void] {
  const [filter, setFilter] = useState<SessionListFilter>(readSessionFilter);
  const update = useCallback((patch: Partial<SessionListFilter>) => {
    setFilter((previous) => {
      const next = { ...previous, ...patch };
      writeSessionFilter(next);
      return next;
    });
  }, []);
  return [filter, update];
}

/** `sessions` narrowed by the filter (client side, over the rows already loaded). */
export function applySessionFilter(sessions: SessionCard[], filter: SessionListFilter): SessionCard[] {
  if (!isFilterOn(filter)) return sessions;
  const byAgent = filterSessionsByHarness(sessions, filter.hiddenHarnesses);
  const busy = new Set(byAgent.filter((session) => session.busy).map((session) => session.id));
  return filterSessionsByStatus(byAgent, { ...DEFAULT_SESSION_STATUS_FILTER, working: filter.working }, busy, new Set(), new Set());
}

/** A worktree of a project, as `project.worktrees` lists it. */
export interface Worktree {
  path: string;
  name: string;
}

const trimSlashes = (path: string) => path.replace(/\/+$/, "");

/**
 * Whether a session works in `worktree`; any does without one. An older desktop does not say where a
 * session works, so it goes by the branch, which only one working copy can have checked out.
 */
export function inWorktree(session: Pick<SessionCard, "worktreePath" | "branch">, worktree: Worktree | null | undefined): boolean {
  if (!worktree) return true;
  if (session.worktreePath) return trimSlashes(session.worktreePath) === trimSlashes(worktree.path);
  return session.branch === worktree.name;
}
