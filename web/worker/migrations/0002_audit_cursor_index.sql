-- 0002_audit_cursor_index: GET /api/audit pages one user's events by the cursor (ts, id), newest first. The old
-- (actor_user_id, ts) index cannot order by the id tie-breaker, so it is replaced by (actor_user_id, ts, id), which
-- serves the same lookups. Forward-only: 0001_init.sql is not edited.

CREATE INDEX `audit_log_actor_user_id_ts_id_idx` ON `audit_log` (`actor_user_id`,`ts`,`id`);
DROP INDEX `audit_log_actor_user_id_ts_idx`;
