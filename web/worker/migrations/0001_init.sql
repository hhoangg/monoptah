-- 0001_init: the whole Monoptah web schema (architecture.md 4.3), contract (d) of Wave 0.
-- Generated from src/db/schema.ts with drizzle-kit, then numbered by hand. Forward-only: later
-- changes are new numbered files allocated by WS-A (workstreams.md 2.3), never edits of this one.
-- test/schema.test.ts fails if this file and schema.ts disagree.

CREATE TABLE `audit_log` (
  `id` text PRIMARY KEY NOT NULL,
  `ts` integer NOT NULL,
  `actor_user_id` text,
  `actor_kind` text NOT NULL,
  `action` text NOT NULL,
  `target_type` text,
  `target_id` text,
  `meta` text DEFAULT '{}' NOT NULL,
  CONSTRAINT "audit_log_actor_kind_check" CHECK(actor_kind IN ('user', 'desktop', 'admin', 'system'))
);

CREATE INDEX `audit_log_ts_idx` ON `audit_log` (`ts`);
CREATE INDEX `audit_log_actor_user_id_ts_idx` ON `audit_log` (`actor_user_id`,`ts`);
CREATE TABLE `challenges` (
  `id` text PRIMARY KEY NOT NULL,
  `purpose` text NOT NULL,
  `user_id` text,
  `challenge` text NOT NULL,
  `payload` text,
  `expires_at` integer NOT NULL,
  FOREIGN KEY (`user_id`) REFERENCES `users`(`id`) ON UPDATE no action ON DELETE cascade,
  CONSTRAINT "challenges_purpose_check" CHECK(purpose IN ('register', 'login', 'reauth', 'addpasskey'))
);

CREATE INDEX `challenges_expires_at_idx` ON `challenges` (`expires_at`);
CREATE TABLE `desktop_tokens` (
  `id` text PRIMARY KEY NOT NULL,
  `desktop_id` text NOT NULL,
  `token_hash` text NOT NULL,
  `scope` text DEFAULT 'relay push grants' NOT NULL,
  `created_at` integer NOT NULL,
  `last_used_at` integer,
  `revoked_at` integer,
  FOREIGN KEY (`desktop_id`) REFERENCES `desktops`(`id`) ON UPDATE no action ON DELETE cascade
);

CREATE UNIQUE INDEX `desktop_tokens_token_hash_unique` ON `desktop_tokens` (`token_hash`);
CREATE INDEX `desktop_tokens_desktop_id_idx` ON `desktop_tokens` (`desktop_id`);
CREATE TABLE `desktops` (
  `id` text PRIMARY KEY NOT NULL,
  `user_id` text NOT NULL,
  `name` text NOT NULL,
  `color` text,
  `public_key` text NOT NULL,
  `fingerprint` text NOT NULL,
  `os` text,
  `app_version` text,
  `created_at` integer NOT NULL,
  `last_seen_at` integer,
  `revoked_at` integer,
  FOREIGN KEY (`user_id`) REFERENCES `users`(`id`) ON UPDATE no action ON DELETE cascade
);

CREATE INDEX `desktops_user_id_idx` ON `desktops` (`user_id`);
CREATE TABLE `device_grants` (
  `device_id` text NOT NULL,
  `desktop_id` text NOT NULL,
  `state` text DEFAULT 'pending' NOT NULL,
  `permission` text,
  `requested_permission` text,
  `confirmed_at` integer,
  `updated_at` integer NOT NULL,
  PRIMARY KEY(`device_id`, `desktop_id`),
  FOREIGN KEY (`device_id`) REFERENCES `devices`(`id`) ON UPDATE no action ON DELETE cascade,
  FOREIGN KEY (`desktop_id`) REFERENCES `desktops`(`id`) ON UPDATE no action ON DELETE cascade,
  CONSTRAINT "device_grants_state_check" CHECK(state IN ('pending', 'confirmed', 'revoked')),
  CONSTRAINT "device_grants_permission_check" CHECK("device_grants"."permission" IS NULL OR permission IN ('read', 'control')),
  CONSTRAINT "device_grants_requested_permission_check" CHECK("device_grants"."requested_permission" IS NULL OR requested_permission IN ('read', 'control'))
);

CREATE INDEX `device_grants_desktop_id_idx` ON `device_grants` (`desktop_id`);
CREATE INDEX `device_grants_state_updated_at_idx` ON `device_grants` (`state`,`updated_at`);
CREATE TABLE `devices` (
  `id` text PRIMARY KEY NOT NULL,
  `user_id` text NOT NULL,
  `name` text NOT NULL,
  `public_key` text NOT NULL,
  `fingerprint` text NOT NULL,
  `created_at` integer NOT NULL,
  `last_seen_at` integer,
  `revoked_at` integer,
  FOREIGN KEY (`user_id`) REFERENCES `users`(`id`) ON UPDATE no action ON DELETE cascade
);

CREATE INDEX `devices_user_id_idx` ON `devices` (`user_id`);
CREATE TABLE `invites` (
  `id` text PRIMARY KEY NOT NULL,
  `code_hash` text NOT NULL,
  `created_by` text,
  `kind` text DEFAULT 'user' NOT NULL,
  `expires_at` integer NOT NULL,
  `used_by` text,
  `used_at` integer,
  `revoked_at` integer,
  `created_at` integer NOT NULL,
  FOREIGN KEY (`created_by`) REFERENCES `users`(`id`) ON UPDATE no action ON DELETE cascade,
  FOREIGN KEY (`used_by`) REFERENCES `users`(`id`) ON UPDATE no action ON DELETE set null,
  CONSTRAINT "invites_kind_check" CHECK(kind IN ('user', 'bootstrap')),
  CONSTRAINT "invites_creator_check" CHECK("invites"."kind" = 'bootstrap' OR "invites"."created_by" IS NOT NULL)
);

CREATE UNIQUE INDEX `invites_code_hash_unique` ON `invites` (`code_hash`);
CREATE INDEX `invites_created_by_idx` ON `invites` (`created_by`);
CREATE TABLE `pairing_requests` (
  `id` text PRIMARY KEY NOT NULL,
  `user_code` text NOT NULL,
  `device_code_hash` text NOT NULL,
  `challenge` text NOT NULL,
  `desktop_public_key` text NOT NULL,
  `machine_name` text NOT NULL,
  `os` text,
  `confirmation_code` text NOT NULL,
  `status` text DEFAULT 'pending' NOT NULL,
  `user_id` text,
  `desktop_id` text,
  `created_at` integer NOT NULL,
  `expires_at` integer NOT NULL,
  `last_polled_at` integer,
  `ip_hash` text,
  FOREIGN KEY (`user_id`) REFERENCES `users`(`id`) ON UPDATE no action ON DELETE set null,
  FOREIGN KEY (`desktop_id`) REFERENCES `desktops`(`id`) ON UPDATE no action ON DELETE set null,
  CONSTRAINT "pairing_requests_status_check" CHECK(status IN ('pending', 'approved', 'denied', 'expired', 'consumed'))
);

CREATE UNIQUE INDEX `pairing_requests_user_code_unique` ON `pairing_requests` (`user_code`);
CREATE UNIQUE INDEX `pairing_requests_device_code_hash_unique` ON `pairing_requests` (`device_code_hash`);
CREATE INDEX `pairing_requests_expires_at_idx` ON `pairing_requests` (`expires_at`);
CREATE INDEX `pairing_requests_ip_hash_idx` ON `pairing_requests` (`ip_hash`);
CREATE TABLE `passkeys` (
  `id` text PRIMARY KEY NOT NULL,
  `user_id` text NOT NULL,
  `public_key` blob NOT NULL,
  `counter` integer DEFAULT 0 NOT NULL,
  `transports` text DEFAULT '[]' NOT NULL,
  `device_type` text,
  `backed_up` integer DEFAULT 0 NOT NULL,
  `name` text NOT NULL,
  `created_at` integer NOT NULL,
  `last_used_at` integer,
  FOREIGN KEY (`user_id`) REFERENCES `users`(`id`) ON UPDATE no action ON DELETE cascade
);

CREATE INDEX `passkeys_user_id_idx` ON `passkeys` (`user_id`);
CREATE TABLE `push_prefs` (
  `user_id` text NOT NULL,
  `desktop_id` text NOT NULL,
  `enabled` integer DEFAULT 0 NOT NULL,
  `categories` text DEFAULT '{}' NOT NULL,
  PRIMARY KEY(`user_id`, `desktop_id`),
  FOREIGN KEY (`user_id`) REFERENCES `users`(`id`) ON UPDATE no action ON DELETE cascade,
  FOREIGN KEY (`desktop_id`) REFERENCES `desktops`(`id`) ON UPDATE no action ON DELETE cascade
);

CREATE TABLE `push_subscriptions` (
  `id` text PRIMARY KEY NOT NULL,
  `user_id` text NOT NULL,
  `device_id` text NOT NULL,
  `endpoint` text NOT NULL,
  `p256dh` text NOT NULL,
  `auth` text NOT NULL,
  `vapid_key_id` text NOT NULL,
  `ua` text,
  `created_at` integer NOT NULL,
  `last_success_at` integer,
  `failures` integer DEFAULT 0 NOT NULL,
  FOREIGN KEY (`user_id`) REFERENCES `users`(`id`) ON UPDATE no action ON DELETE cascade,
  FOREIGN KEY (`device_id`) REFERENCES `devices`(`id`) ON UPDATE no action ON DELETE cascade
);

CREATE UNIQUE INDEX `push_subscriptions_endpoint_unique` ON `push_subscriptions` (`endpoint`);
CREATE INDEX `push_subscriptions_user_id_idx` ON `push_subscriptions` (`user_id`);
CREATE INDEX `push_subscriptions_device_id_idx` ON `push_subscriptions` (`device_id`);
CREATE TABLE `rate_limits` (
  `key` text NOT NULL,
  `window_start` integer NOT NULL,
  `count` integer DEFAULT 0 NOT NULL,
  PRIMARY KEY(`key`, `window_start`)
);

CREATE INDEX `rate_limits_window_start_idx` ON `rate_limits` (`window_start`);
CREATE TABLE `recovery_codes` (
  `id` text PRIMARY KEY NOT NULL,
  `user_id` text NOT NULL,
  `code_hash` text NOT NULL,
  `batch` integer NOT NULL,
  `used_at` integer,
  `created_at` integer NOT NULL,
  FOREIGN KEY (`user_id`) REFERENCES `users`(`id`) ON UPDATE no action ON DELETE cascade
);

CREATE INDEX `recovery_codes_user_id_idx` ON `recovery_codes` (`user_id`);
CREATE TABLE `sessions` (
  `id_hash` text PRIMARY KEY NOT NULL,
  `user_id` text NOT NULL,
  `created_at` integer NOT NULL,
  `last_seen_at` integer NOT NULL,
  `idle_expires_at` integer NOT NULL,
  `expires_at` integer NOT NULL,
  `reauth_at` integer,
  `amr` text NOT NULL,
  `recovery_only` integer DEFAULT 0 NOT NULL,
  `user_agent` text,
  `revoked_at` integer,
  FOREIGN KEY (`user_id`) REFERENCES `users`(`id`) ON UPDATE no action ON DELETE cascade,
  CONSTRAINT "sessions_amr_check" CHECK(amr IN ('passkey', 'recovery'))
);

CREATE INDEX `sessions_user_id_idx` ON `sessions` (`user_id`);
CREATE INDEX `sessions_idle_expires_at_idx` ON `sessions` (`idle_expires_at`);
CREATE INDEX `sessions_expires_at_idx` ON `sessions` (`expires_at`);
CREATE TABLE `settings` (
  `key` text PRIMARY KEY NOT NULL,
  `value` text NOT NULL,
  `updated_at` integer NOT NULL,
  `updated_by` text,
  FOREIGN KEY (`updated_by`) REFERENCES `users`(`id`) ON UPDATE no action ON DELETE set null
);

CREATE TABLE `users` (
  `id` text PRIMARY KEY NOT NULL,
  `handle` text NOT NULL,
  `display_name` text NOT NULL,
  `role` text DEFAULT 'user' NOT NULL,
  `status` text DEFAULT 'active' NOT NULL,
  `invited_by` text,
  `invite_id` text,
  `invites_granted` integer DEFAULT 3 NOT NULL,
  `blocked_at` integer,
  `blocked_reason` text,
  `blocked_via` text,
  `recovery_ack_at` integer,
  `created_at` integer NOT NULL,
  FOREIGN KEY (`invite_id`) REFERENCES `invites`(`id`) ON UPDATE no action ON DELETE set null,
  CONSTRAINT "users_handle_check" CHECK(length("users"."handle") BETWEEN 3 AND 32 AND "users"."handle" = lower("users"."handle")),
  CONSTRAINT "users_role_check" CHECK(role IN ('user', 'admin')),
  CONSTRAINT "users_status_check" CHECK(status IN ('active', 'blocked'))
);

CREATE UNIQUE INDEX `users_handle_unique` ON `users` (`handle`);
CREATE INDEX `users_invited_by_idx` ON `users` (`invited_by`);
