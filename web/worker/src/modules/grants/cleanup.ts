import { DAY, RETENTION, runCleanupQueries, type Cleanup, type CleanupQuery } from "../cleanup.ts";

/** A grant that stayed `pending` for 7 days expires (R-15). Uses `device_grants_state_updated_at_idx`. */
export const PENDING_GRANT_TTL_SECONDS = 7 * DAY;

export const CLEANUP_QUERIES: readonly CleanupQuery[] = [
  {
    name: "pending device_grants",
    sql: "DELETE FROM device_grants WHERE state = 'pending' AND updated_at < ?1",
    bind: (now) => [now - PENDING_GRANT_TTL_SECONDS],
  },
  {
    // Expired tickets, used or not (a used ticket expires 120 s after it was made). Uses `selection_tickets_expires_at_idx`.
    name: "selection_tickets",
    sql: "DELETE FROM selection_tickets WHERE expires_at < ?1",
    bind: (now) => [now - RETENTION.selectionTickets],
  },
  {
    // Same for terminal tickets. Uses `terminal_tickets_expires_at_idx`.
    name: "terminal_tickets",
    sql: "DELETE FROM terminal_tickets WHERE expires_at < ?1",
    bind: (now) => [now - RETENTION.terminalTickets],
  },
];

export const cleanup: Cleanup = (env, now) => runCleanupQueries(env.DB, CLEANUP_QUERIES, now);
