import { RETENTION, runCleanupQueries, type Cleanup, type CleanupQuery } from "../cleanup.ts";

/** Audit rows older than the 180 day retention (security.md 10). Uses `audit_log_ts_id_idx` (prefix `ts`). */
export const CLEANUP_QUERIES: readonly CleanupQuery[] = [
  {
    name: "audit_log",
    sql: "DELETE FROM audit_log WHERE ts < ?",
    bind: (now) => [now - RETENTION.auditLog],
  },
];

export const cleanup: Cleanup = (env, now) => runCleanupQueries(env.DB, CLEANUP_QUERIES, now);
