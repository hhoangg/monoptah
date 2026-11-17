// The general authenticated limit of security.md 3.5: 120 calls per session per minute, counted in memory.
// Best effort: every isolate has its own counters, so the effective limit can be higher when requests are
// spread over isolates, and a counter is lost when the isolate is recycled. In exchange it writes nothing
// to D1 (the free plan has 100,000 writes per day).
import { nowSeconds } from "../../lib/time.ts";

export const SESSION_LIMIT = 120;
export const SESSION_WINDOW_SECONDS = 60;
/** Upper bound of tracked sessions per isolate; the table is purged and then trimmed beyond it. */
export const MAX_TRACKED_SESSIONS = 5000;

interface Counter {
  windowStart: number;
  count: number;
}

const counters = new Map<string, Counter>();

export interface MemoryLimitResult {
  ok: boolean;
  retryAfter: number;
  /** True once per session and window, on the call that first exceeds the limit. */
  firstBreach: boolean;
}

/** Counts one call of the session `idHash` (fixed window of 60 s, in memory). */
export function consumeSession(idHash: string, now: number = nowSeconds()): MemoryLimitResult {
  const windowStart = now - (now % SESSION_WINDOW_SECONDS);
  let counter = counters.get(idHash);
  if (!counter || counter.windowStart !== windowStart) {
    if (!counter && counters.size >= MAX_TRACKED_SESSIONS) makeRoom(windowStart);
    counter = { windowStart, count: 0 };
    counters.set(idHash, counter);
  }
  counter.count += 1;
  return {
    ok: counter.count <= SESSION_LIMIT,
    retryAfter: Math.max(1, windowStart + SESSION_WINDOW_SECONDS - now),
    firstBreach: counter.count === SESSION_LIMIT + 1,
  };
}

/** Drops counters of past windows, then the oldest entries, so the map never exceeds its bound. */
function makeRoom(currentWindow: number): void {
  for (const [key, counter] of counters) {
    if (counter.windowStart < currentWindow) counters.delete(key);
  }
  // Map iterates in insertion order: the first keys are the oldest.
  while (counters.size >= MAX_TRACKED_SESSIONS) {
    const oldest = counters.keys().next();
    if (oldest.done) break;
    counters.delete(oldest.value);
  }
}

/** Number of tracked sessions (tests). */
export function trackedSessions(): number {
  return counters.size;
}

/** Forgets every counter (tests). */
export function resetSessionLimits(): void {
  counters.clear();
}
