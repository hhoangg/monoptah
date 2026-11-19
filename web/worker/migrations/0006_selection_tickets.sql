-- 0006_selection_tickets: single-use tickets that prove a fresh passkey re-auth to a desktop, so a web device with
-- Control access can widen what it may see (security.md decision "Selection widening from the web"). Only the
-- SHA-256 of the ticket is stored. A ticket is bound to one user, desktop and device and lives 120 seconds; the
-- desktop consumes it (`used_at`) through `POST /api/desktop/grants/:deviceId/selection-ticket`. Expired rows are
-- removed by the daily cleanup. Forward-only.

CREATE TABLE `selection_tickets` (
  `token_hash` text PRIMARY KEY NOT NULL,
  `user_id` text NOT NULL,
  `desktop_id` text NOT NULL,
  `device_id` text NOT NULL,
  `created_at` integer NOT NULL,
  `expires_at` integer NOT NULL,
  `used_at` integer,
  FOREIGN KEY (`user_id`) REFERENCES `users`(`id`) ON UPDATE no action ON DELETE cascade,
  FOREIGN KEY (`desktop_id`) REFERENCES `desktops`(`id`) ON UPDATE no action ON DELETE cascade,
  FOREIGN KEY (`device_id`) REFERENCES `devices`(`id`) ON UPDATE no action ON DELETE cascade
);

CREATE INDEX `selection_tickets_user_id_idx` ON `selection_tickets` (`user_id`);
CREATE INDEX `selection_tickets_desktop_id_idx` ON `selection_tickets` (`desktop_id`);
CREATE INDEX `selection_tickets_device_id_idx` ON `selection_tickets` (`device_id`);
CREATE INDEX `selection_tickets_expires_at_idx` ON `selection_tickets` (`expires_at`);
