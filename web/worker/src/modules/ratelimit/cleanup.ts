import { RETENTION, runCleanupQueries, type Cleanup, type CleanupQuery } from "../cleanup.ts";

/** Fixed windows that started more than a day ago (security.md 10). Uses `rate_limits_window_start_idx`. */
export const CLEANUP_QUERIES: readonly CleanupQuery[] = [
  {
    name: "rate_limits",
    sql: "DELETE FROM rate_limits WHERE window_start < ?",
    bind: (now) => [now - RETENTION.rateLimits],
  },
];

export const cleanup: Cleanup = (env, now) => runCleanupQueries(env.DB, CLEANUP_QUERIES, now);
