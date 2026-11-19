import { RETENTION, runCleanupQueries, type Cleanup, type CleanupQuery } from "../cleanup.ts";

/**
 * Sessions past their absolute or idle expiry plus 7 days (security.md 10). Uses the two indexes
 * `sessions_expires_at_idx` and `sessions_idle_expires_at_idx` (a multi-index OR).
 */
export const CLEANUP_QUERIES: readonly CleanupQuery[] = [
  {
    name: "sessions",
    sql: "DELETE FROM sessions WHERE expires_at < ?1 OR idle_expires_at < ?1",
    bind: (now) => [now - RETENTION.sessions],
  },
];

export const cleanup: Cleanup = (env, now) => runCleanupQueries(env.DB, CLEANUP_QUERIES, now);
