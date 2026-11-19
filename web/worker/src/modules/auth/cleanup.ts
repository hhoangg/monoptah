import { RETENTION, runCleanupQueries, type Cleanup, type CleanupQuery } from "../cleanup.ts";

/** Expired WebAuthn challenges (5 minute life, a grace of 1 hour; security.md 10). Uses `challenges_expires_at_idx`. */
export const CLEANUP_QUERIES: readonly CleanupQuery[] = [
  {
    name: "challenges",
    sql: "DELETE FROM challenges WHERE expires_at < ?",
    bind: (now) => [now - RETENTION.challenges],
  },
];

export const cleanup: Cleanup = (env, now) => runCleanupQueries(env.DB, CLEANUP_QUERIES, now);
