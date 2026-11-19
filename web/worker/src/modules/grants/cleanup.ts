import { DAY, runCleanupQueries, type Cleanup, type CleanupQuery } from "../cleanup.ts";

/** A grant that stayed `pending` for 7 days expires (R-15). Uses `device_grants_state_updated_at_idx`. */
export const PENDING_GRANT_TTL_SECONDS = 7 * DAY;

export const CLEANUP_QUERIES: readonly CleanupQuery[] = [
  {
    name: "pending device_grants",
    sql: "DELETE FROM device_grants WHERE state = 'pending' AND updated_at < ?1",
    bind: (now) => [now - PENDING_GRANT_TTL_SECONDS],
  },
];

export const cleanup: Cleanup = (env, now) => runCleanupQueries(env.DB, CLEANUP_QUERIES, now);
