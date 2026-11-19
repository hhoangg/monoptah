// Shared shape of the daily cleanup (cron.ts). Every `modules/<m>/cleanup.ts` exports one `cleanup`.
import type { Env } from "../env.ts";

export interface CleanupQuery {
  /** Short name for tests and logs. */
  name: string;
  /** A `DELETE` that uses an index (test/cleanup.test.ts checks the query plan). */
  sql: string;
  /** Positional parameters for `now` (unix seconds). */
  bind: (now: number) => unknown[];
}

export type Cleanup = (env: Env, now: number) => Promise<void>;

/** Retention, in seconds (security.md 10). */
export const HOUR = 3600;
export const DAY = 86400;
export const RETENTION = {
  /** `challenges`: removed 1 hour after they expire. */
  challenges: HOUR,
  /** `sessions`: removed 7 days after the absolute or the idle expiry. */
  sessions: 7 * DAY,
  /** `rate_limits`: removed 1 day after the window started. */
  rateLimits: DAY,
  /** `audit_log`: 180 days. */
  auditLog: 180 * DAY,
} as const;

/** Runs the queries as one `db.batch` (a batch counts as one query against the subrequest cap). */
export async function runCleanupQueries(db: D1Database, queries: readonly CleanupQuery[], now: number): Promise<void> {
  if (queries.length === 0) return;
  await db.batch(queries.map((q) => db.prepare(q.sql).bind(...q.bind(now))));
}

/** For modules whose rows are cleaned by another workstream later (pairing, grants, push). */
export const noCleanup: Cleanup = async () => {};
