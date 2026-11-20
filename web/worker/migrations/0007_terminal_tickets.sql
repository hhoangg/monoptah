-- 0007_terminal_tickets: single-use tickets that prove a fresh passkey re-auth to a desktop before it opens a web
-- terminal (security.md T20 and "Terminal"). Same shape as `selection_tickets` plus the project the terminal opens
-- in. Only the SHA-256 of the ticket is stored. A ticket is bound to one user, desktop, device and project and lives
-- 60 seconds; the desktop consumes it (`used_at`) through `POST /api/desktop/grants/:deviceId/terminal-ticket`.
-- Expired rows are removed by the daily cleanup. Forward-only.

CREATE TABLE `terminal_tickets` (
  `token_hash` text PRIMARY KEY NOT NULL,
  `user_id` text NOT NULL,
  `desktop_id` text NOT NULL,
  `device_id` text NOT NULL,
  `project_id` text NOT NULL,
  `created_at` integer NOT NULL,
  `expires_at` integer NOT NULL,
  `used_at` integer,
  FOREIGN KEY (`user_id`) REFERENCES `users`(`id`) ON UPDATE no action ON DELETE cascade,
  FOREIGN KEY (`desktop_id`) REFERENCES `desktops`(`id`) ON UPDATE no action ON DELETE cascade,
  FOREIGN KEY (`device_id`) REFERENCES `devices`(`id`) ON UPDATE no action ON DELETE cascade
);

CREATE INDEX `terminal_tickets_user_id_idx` ON `terminal_tickets` (`user_id`);
CREATE INDEX `terminal_tickets_desktop_id_idx` ON `terminal_tickets` (`desktop_id`);
CREATE INDEX `terminal_tickets_device_id_idx` ON `terminal_tickets` (`device_id`);
CREATE INDEX `terminal_tickets_expires_at_idx` ON `terminal_tickets` (`expires_at`);
