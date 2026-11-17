import { DAY, runCleanupQueries, type Cleanup, type CleanupQuery } from "../cleanup.ts";

/** `pairing_requests` are deleted 24 hours after they expired (resolved ones expire at most 10 minutes after creation). */
export const CLEANUP_QUERIES: readonly CleanupQuery[] = [
  {
    name: "pairing_requests",
    sql: "DELETE FROM pairing_requests WHERE expires_at < ?1",
    bind: (now) => [now - DAY],
  },
];

export const cleanup: Cleanup = (env, now) => runCleanupQueries(env.DB, CLEANUP_QUERIES, now);
