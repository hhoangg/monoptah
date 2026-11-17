-- 0003_admin_indexes (A-09): indexes for the foreign keys that had none, and for the admin lists.
-- Foreign keys to `users` and `desktops` without an index make SQLite scan the child table whenever a parent row
-- is deleted (account deletion) and make the `ON DELETE` actions slow: settings.updated_by,
-- pairing_requests.user_id, pairing_requests.desktop_id, invites.used_by, challenges.user_id, users.invite_id and push_prefs.desktop_id
-- (the last two found while adding the schema test that every foreign key column is indexed: deleting an invite
-- scans `users`, deleting a desktop scans `push_prefs`).
-- `GET /api/admin/users` lists every user by creation time (users_created_at_idx, no temporary sort), and
-- `GET /api/admin/audit` pages all events by the cursor (ts, id) newest first: (ts, id) replaces the (ts) index,
-- which it serves as a prefix (the 180 day retention cleanup `ts < ?`). Forward-only: 0001 and 0002 are not edited.

CREATE INDEX `settings_updated_by_idx` ON `settings` (`updated_by`);
CREATE INDEX `pairing_requests_user_id_idx` ON `pairing_requests` (`user_id`);
CREATE INDEX `pairing_requests_desktop_id_idx` ON `pairing_requests` (`desktop_id`);
CREATE INDEX `invites_used_by_idx` ON `invites` (`used_by`);
CREATE INDEX `challenges_user_id_idx` ON `challenges` (`user_id`);
CREATE INDEX `push_prefs_desktop_id_idx` ON `push_prefs` (`desktop_id`);
CREATE INDEX `users_invite_id_idx` ON `users` (`invite_id`);
CREATE INDEX `users_created_at_idx` ON `users` (`created_at`,`id`);
CREATE INDEX `audit_log_ts_id_idx` ON `audit_log` (`ts`,`id`);
DROP INDEX `audit_log_ts_idx`;
