// In-memory limits of security.md 3.5: the general authenticated limit (120 calls per session per minute), the separate
// avatar bucket (300 per session per minute) and the per-isolate pre-filter in front of the D1 per-IP limits.
// Best effort: every isolate has its own counters, so the effective limit can be higher when requests are
// spread over isolates, and a counter is lost when the isolate is recycled. In exchange it writes nothing
// to D1 (the free plan has 100,000 writes per day).
import { nowSeconds } from "../../lib/time.ts";

export const SESSION_LIMIT = 120;
export const SESSION_WINDOW_SECONDS = 60;
/** Avatar GETs have their own bucket, so a page of avatars does not eat the general limit. */
export const AVATAR_SESSION_LIMIT = 300;
export const AVATAR_ROUTE_KEY = "GET /api/desktops/:id/avatar";
/** Upper bound of tracked sessions per isolate; the table is purged and then trimmed beyond it. */
export const MAX_TRACKED_SESSIONS = 5000;
/** Upper bound of tracked (group, IP) keys per isolate, with the same purge and trim. */
export const MAX_TRACKED_IPS = 5000;

interface Counter {
  windowStart: number;
  /** First second after the window; the counter is stale from then on. */
  windowEnd: number;
  count: number;
}

export interface MemoryLimitResult {
  ok: boolean;
  retryAfter: number;
  /** True once per key and window, on the call that first exceeds the limit. */
  firstBreach: boolean;
}

/** A bounded table of fixed-window counters, one per key. */
class MemoryCounters {
  private readonly counters = new Map<string, Counter>();

  private readonly maxTracked: number;

  constructor(maxTracked: number) {
    this.maxTracked = maxTracked;
  }

  consume(key: string, limit: number, windowSeconds: number, now: number): MemoryLimitResult {
    const windowStart = now - (now % windowSeconds);
    let counter = this.counters.get(key);
    if (!counter || counter.windowStart !== windowStart) {
      if (!counter && this.counters.size >= this.maxTracked) this.makeRoom(now);
      counter = { windowStart, windowEnd: windowStart + windowSeconds, count: 0 };
      this.counters.set(key, counter);
    }
    counter.count += 1;
    return {
      ok: counter.count <= limit,
      retryAfter: Math.max(1, windowStart + windowSeconds - now),
      firstBreach: counter.count === limit + 1,
    };
  }

  /**
   * Drops counters whose own window has ended (`windowEnd <= now`), whatever the window of the key being inserted,
   * so a short-window key never evicts a live long-window counter. If the table is still full after that, every
   * entry is live: the oldest inserted ones are dropped (losing a live counter, which only makes the best-effort
   * limit looser) so the map never exceeds its bound.
   */
  private makeRoom(now: number): void {
    for (const [key, counter] of this.counters) {
      if (counter.windowEnd <= now) this.counters.delete(key);
    }
    // Map iterates in insertion order: the first keys are the oldest.
    while (this.counters.size >= this.maxTracked) {
      const oldest = this.counters.keys().next();
      if (oldest.done) break;
      this.counters.delete(oldest.value);
    }
  }

  get size(): number {
    return this.counters.size;
  }

  clear(): void {
    this.counters.clear();
  }
}

const sessions = new MemoryCounters(MAX_TRACKED_SESSIONS);
const avatarSessions = new MemoryCounters(MAX_TRACKED_SESSIONS);
const ips = new MemoryCounters(MAX_TRACKED_IPS);

/** Counts one call of the session `idHash` (fixed window of 60 s, in memory). */
export function consumeSession(idHash: string, now: number = nowSeconds()): MemoryLimitResult {
  return sessions.consume(idHash, SESSION_LIMIT, SESSION_WINDOW_SECONDS, now);
}

/** Counts one avatar GET of the session `idHash` (own bucket: 300 per 60 s, in memory). */
export function consumeAvatarSession(idHash: string, now: number = nowSeconds()): MemoryLimitResult {
  return avatarSessions.consume(idHash, AVATAR_SESSION_LIMIT, SESSION_WINDOW_SECONDS, now);
}

/**
 * Pre-filter in front of the D1 per-IP limits: counts one call under `key` (`group:ip:<IP hash>`) with the same
 * limit and window as the D1 limit. Best effort like the session limit: per isolate, lost on recycle, and the
 * table is bounded. A refusal here lets the caller answer 429 without touching D1.
 */
export function consumeIp(key: string, limit: number, windowSeconds: number, now: number = nowSeconds()): MemoryLimitResult {
  return ips.consume(key, limit, windowSeconds, now);
}

/** Number of tracked sessions (tests). */
export function trackedSessions(): number {
  return sessions.size;
}

/** Number of tracked IP keys (tests). */
export function trackedIps(): number {
  return ips.size;
}

/** Forgets every session counter (tests). */
export function resetSessionLimits(): void {
  sessions.clear();
  avatarSessions.clear();
}

/** Forgets every IP pre-filter counter (tests). */
export function resetIpLimits(): void {
  ips.clear();
}
