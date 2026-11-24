-- 0009_mono_reset_tickets: single-use tickets that prove a fresh passkey re-auth to a desktop before it resets a Mono's
-- conversation for a web device (security.md T20 and "Mono reset"). Same shape as `terminal_tickets` with the Mono the
-- reset targets. Only the SHA-256 of the ticket is stored. A ticket is bound to one user, desktop, device and Mono and
-- lives 60 seconds; the desktop consumes it (`used_at`) through `POST /api/desktop/grants/:deviceId/mono-reset-ticket`.
-- Expired rows are removed by the daily cleanup. Forward-only.

CREATE TABLE `mono_reset_tickets` (
  `token_hash` text PRIMARY KEY NOT NULL,
  `user_id` text NOT NULL,
  `desktop_id` text NOT NULL,
  `device_id` text NOT NULL,
  `mono_id` text NOT NULL,
  `created_at` integer NOT NULL,
  `expires_at` integer NOT NULL,
  `used_at` integer,
  FOREIGN KEY (`user_id`) REFERENCES `users`(`id`) ON UPDATE no action ON DELETE cascade,
  FOREIGN KEY (`desktop_id`) REFERENCES `desktops`(`id`) ON UPDATE no action ON DELETE cascade,
  FOREIGN KEY (`device_id`) REFERENCES `devices`(`id`) ON UPDATE no action ON DELETE cascade
);

CREATE INDEX `mono_reset_tickets_user_id_idx` ON `mono_reset_tickets` (`user_id`);
CREATE INDEX `mono_reset_tickets_desktop_id_idx` ON `mono_reset_tickets` (`desktop_id`);
CREATE INDEX `mono_reset_tickets_device_id_idx` ON `mono_reset_tickets` (`device_id`);
CREATE INDEX `mono_reset_tickets_expires_at_idx` ON `mono_reset_tickets` (`expires_at`);
