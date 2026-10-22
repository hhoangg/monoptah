import type { WorkflowId } from "../../features/workflows/model/workflow";

/**
 * Which workflow runs are going, across windows. A run lives in the memory of the window that
 * started it, but the window that drives workflows can change while it goes (sleep, a window
 * closing, the lease being taken over). Each window writes an entry for every run it is
 * driving, refreshes it while the run is live and removes it when the run ends. Everything
 * that would touch a run another window may still be driving (sweeping it to "interrupted",
 * resuming it, starting the workflow again, clearing a stop meant for it) checks here first.
 * An entry nobody refreshed for 30 s belongs to a window that is gone.
 */
export const LIVE_RUNS_KEY = "monocode.workflowLiveRuns";
export const LIVE_RUN_REFRESH_MS = 10_000;
export const LIVE_RUN_STALE_MS = 30_000;

type Entry = { workflowId: WorkflowId; owner: string; at: number };
type RegistryStorage = Pick<Storage, "getItem" | "setItem">;

export type LiveRunRegistry = {
  /** Records (or refreshes) a run this window is driving. */
  touch(runId: string, workflowId: WorkflowId): void;
  /** Forgets a run this window was driving. */
  remove(runId: string): void;
  /** Forgets every run this window was driving, for a window that is going away. */
  removeOwned(): void;
  /** Whether some window, this one included, refreshed the run's entry recently. */
  isFresh(runId: string): boolean;
  /** The id of a run of the workflow that some window is driving, if any. */
  freshRunOf(workflowId: WorkflowId): string | undefined;
};

export function createLiveRunRegistry(options: {
  storage: RegistryStorage;
  owner?: string;
  now?: () => number;
}): LiveRunRegistry {
  const { storage } = options;
  const owner = options.owner ?? crypto.randomUUID();
  const now = options.now ?? Date.now;
  const read = (): Record<string, Entry> => {
    try {
      const value = JSON.parse(storage.getItem(LIVE_RUNS_KEY) ?? "{}");
      return value && typeof value === "object" && !Array.isArray(value)
        ? value
        : {};
    } catch {
      return {};
    }
  };
  const fresh = (entry: Entry | undefined): entry is Entry =>
    !!entry &&
    typeof entry.at === "number" &&
    now() - entry.at <= LIVE_RUN_STALE_MS;
  /** Writes back without entries that went stale, so the map does not grow. */
  const write = (entries: Record<string, Entry>) => {
    try {
      const kept = Object.fromEntries(
        Object.entries(entries).filter(([, entry]) => fresh(entry)),
      );
      storage.setItem(LIVE_RUNS_KEY, JSON.stringify(kept));
    } catch {
      // Without the registry other windows fall back to what their own memory knows.
    }
  };
  return {
    touch(runId, workflowId) {
      write({ ...read(), [runId]: { workflowId, owner, at: now() } });
    },
    remove(runId) {
      const entries = read();
      if (entries[runId]?.owner !== owner) return;
      delete entries[runId];
      write(entries);
    },
    removeOwned() {
      const entries = read();
      for (const [runId, entry] of Object.entries(entries))
        if (entry.owner === owner) delete entries[runId];
      write(entries);
    },
    isFresh: (runId) => fresh(read()[runId]),
    freshRunOf(workflowId) {
      return Object.entries(read()).find(
        ([, entry]) => fresh(entry) && entry.workflowId === workflowId,
      )?.[0];
    },
  };
}

let shared: LiveRunRegistry | null | undefined;

/** This window's registry, or none where localStorage is missing or throws. */
export function liveRunRegistry(): LiveRunRegistry | null {
  if (shared === undefined) {
    try {
      shared = createLiveRunRegistry({ storage: localStorage });
    } catch {
      shared = null;
    }
  }
  return shared;
}
