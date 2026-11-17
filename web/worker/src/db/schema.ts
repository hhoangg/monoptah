// D1 schema (architecture.md 4.3). Contract (d), workstreams.md 2.1: this file and
// migrations/0001_init.sql land together and are frozen after Wave 0. A change is a new
// forward-only migration allocated by WS-A (workstreams.md 2.3), never an edit of 0001.
//
// Conventions: ids are random UUIDv4 text unless noted; times are unix seconds (integer);
// booleans are 0/1 integers; JSON columns are text. Secrets are stored only as hashes.
// A test (test/schema.test.ts) applies the migration and compares it with this file, so the
// two cannot drift.
import { sql } from "drizzle-orm";
import {
  check,
  customType,
  index,
  integer,
  primaryKey,
  sqliteTable,
  text,
  type AnySQLiteColumn,
} from "drizzle-orm/sqlite-core";

/** BLOB column exposed as `Uint8Array` (D1 returns an `ArrayBuffer` or a number array). */
const bytes = customType<{ data: Uint8Array; driverData: ArrayBuffer | number[] }>({
  dataType: () => "blob",
  toDriver: (value) => value.slice().buffer as ArrayBuffer,
  fromDriver: (value) => new Uint8Array(value),
});

/** Boolean stored as 0/1. Callers add their own `.default(sql`0`)`/`.default(sql`1`)` where one is needed. */
const bool = (name: string) => integer(name, { mode: "boolean" });
const json = <T>(name: string) => text(name, { mode: "json" }).$type<T>();

/** `CHECK (col IN ('a','b'))` for an enum-like text column. */
const oneOf = (column: string, values: readonly string[]) =>
  sql.raw(`${column} IN (${values.map((v) => `'${v}'`).join(", ")})`);

export const USER_ROLES = ["user", "admin"] as const;
export const USER_STATUSES = ["active", "blocked"] as const;
export const CHALLENGE_PURPOSES = ["register", "login", "reauth", "addpasskey"] as const;
export const SESSION_AMRS = ["passkey", "recovery"] as const;
export const INVITE_KINDS = ["user", "bootstrap"] as const;
export const GRANT_STATES = ["pending", "confirmed", "revoked"] as const;
export const PERMISSIONS = ["read", "control"] as const;
export const PAIRING_STATUSES = ["pending", "approved", "denied", "expired", "consumed"] as const;
export const AUDIT_ACTOR_KINDS = ["user", "desktop", "admin", "system"] as const;

// --- Accounts ---

export const users = sqliteTable(
  "users",
  {
    id: text("id").primaryKey(),
    /** 3-32 chars, lowercase, immutable. */
    handle: text("handle").notNull().unique(),
    displayName: text("display_name").notNull(),
    role: text("role", { enum: USER_ROLES }).notNull().default("user"),
    status: text("status", { enum: USER_STATUSES }).notNull().default("active"),
    /**
     * The inviter's user id, null for the bootstrap admin. Plain text, no FK action (like `blockedVia`):
     * account deletion keeps it, so the invite tree survives the removal of a row.
     */
    invitedBy: text("invited_by"),
    inviteId: text("invite_id").references((): AnySQLiteColumn => invites.id, { onDelete: "set null" }),
    invitesGranted: integer("invites_granted").notNull().default(3),
    blockedAt: integer("blocked_at"),
    blockedReason: text("blocked_reason"),
    /** User id of the branch root that cascaded the block, or null. Plain text, no FK: it outlives the root's row. */
    blockedVia: text("blocked_via"),
    /** Set when the user confirms "I saved these"; null at the next sign-in forces a regenerate. */
    recoveryAckAt: integer("recovery_ack_at"),
    createdAt: integer("created_at").notNull(),
  },
  (t) => [
    check("users_handle_check", sql`length(${t.handle}) BETWEEN 3 AND 32 AND ${t.handle} = lower(${t.handle})`),
    check("users_role_check", oneOf("role", USER_ROLES)),
    check("users_status_check", oneOf("status", USER_STATUSES)),
    index("users_invited_by_idx").on(t.invitedBy),
  ],
);

export const passkeys = sqliteTable(
  "passkeys",
  {
    /** WebAuthn credential id, base64url. */
    id: text("id").primaryKey(),
    userId: text("user_id")
      .notNull()
      .references(() => users.id, { onDelete: "cascade" }),
    publicKey: bytes("public_key").notNull(),
    counter: integer("counter").notNull().default(0),
    transports: json<string[]>("transports").notNull().default(sql`'[]'`),
    deviceType: text("device_type"),
    backedUp: bool("backed_up").notNull().default(sql`0`),
    name: text("name").notNull(),
    createdAt: integer("created_at").notNull(),
    lastUsedAt: integer("last_used_at"),
  },
  (t) => [index("passkeys_user_id_idx").on(t.userId)],
);

export const challenges = sqliteTable(
  "challenges",
  {
    id: text("id").primaryKey(),
    purpose: text("purpose", { enum: CHALLENGE_PURPOSES }).notNull(),
    userId: text("user_id").references(() => users.id, { onDelete: "cascade" }),
    challenge: text("challenge").notNull(),
    /** JSON (invite, handle). */
    payload: json<Record<string, unknown>>("payload"),
    /** 5 minutes after creation; a challenge is consumed (deleted) on use. */
    expiresAt: integer("expires_at").notNull(),
  },
  (t) => [
    check("challenges_purpose_check", oneOf("purpose", CHALLENGE_PURPOSES)),
    index("challenges_expires_at_idx").on(t.expiresAt),
  ],
);

export const recoveryCodes = sqliteTable(
  "recovery_codes",
  {
    id: text("id").primaryKey(),
    userId: text("user_id")
      .notNull()
      .references(() => users.id, { onDelete: "cascade" }),
    /** HMAC-SHA-256 with RECOVERY_PEPPER, hex. */
    codeHash: text("code_hash").notNull(),
    batch: integer("batch").notNull(),
    usedAt: integer("used_at"),
    createdAt: integer("created_at").notNull(),
  },
  (t) => [index("recovery_codes_user_id_idx").on(t.userId)],
);

export const sessions = sqliteTable(
  "sessions",
  {
    /** SHA-256 of the cookie token, hex. */
    idHash: text("id_hash").primaryKey(),
    userId: text("user_id")
      .notNull()
      .references(() => users.id, { onDelete: "cascade" }),
    createdAt: integer("created_at").notNull(),
    lastSeenAt: integer("last_seen_at").notNull(),
    /** Idle timeout (7 days), extended on use at most once per hour. */
    idleExpiresAt: integer("idle_expires_at").notNull(),
    /** Absolute lifetime (30 days; 30 minutes for recovery_only). */
    expiresAt: integer("expires_at").notNull(),
    reauthAt: integer("reauth_at"),
    amr: text("amr", { enum: SESSION_AMRS }).notNull(),
    recoveryOnly: bool("recovery_only").notNull().default(sql`0`),
    /** Truncated. */
    userAgent: text("user_agent"),
    revokedAt: integer("revoked_at"),
  },
  (t) => [
    check("sessions_amr_check", oneOf("amr", SESSION_AMRS)),
    index("sessions_user_id_idx").on(t.userId),
    index("sessions_idle_expires_at_idx").on(t.idleExpiresAt),
    // Not in architecture.md 4.3: the daily cleanup deletes on `expires_at OR idle_expires_at`.
    index("sessions_expires_at_idx").on(t.expiresAt),
  ],
);

export const invites = sqliteTable(
  "invites",
  {
    id: text("id").primaryKey(),
    /** SHA-256 of the code, hex. Unique so the registration lookup is an index search. */
    codeHash: text("code_hash").notNull().unique(),
    /** Null only for a `bootstrap` invite, which no user created (the first admin, A-10). */
    createdBy: text("created_by").references((): AnySQLiteColumn => users.id, { onDelete: "cascade" }),
    kind: text("kind", { enum: INVITE_KINDS }).notNull().default("user"),
    expiresAt: integer("expires_at").notNull(),
    usedBy: text("used_by").references((): AnySQLiteColumn => users.id, { onDelete: "set null" }),
    usedAt: integer("used_at"),
    revokedAt: integer("revoked_at"),
    createdAt: integer("created_at").notNull(),
  },
  (t) => [
    check("invites_kind_check", oneOf("kind", INVITE_KINDS)),
    check("invites_creator_check", sql`${t.kind} = 'bootstrap' OR ${t.createdBy} IS NOT NULL`),
    index("invites_created_by_idx").on(t.createdBy),
  ],
);

// --- Desktops and devices ---

export const desktops = sqliteTable(
  "desktops",
  {
    id: text("id").primaryKey(),
    userId: text("user_id")
      .notNull()
      .references(() => users.id, { onDelete: "cascade" }),
    name: text("name").notNull(),
    color: text("color"),
    /** X25519, base64url. */
    publicKey: text("public_key").notNull(),
    /** Hex SHA-256 of the public key. */
    fingerprint: text("fingerprint").notNull(),
    os: text("os"),
    appVersion: text("app_version"),
    createdAt: integer("created_at").notNull(),
    /** Written on desktop disconnect only, to spare the D1 write budget. */
    lastSeenAt: integer("last_seen_at"),
    revokedAt: integer("revoked_at"),
  },
  (t) => [index("desktops_user_id_idx").on(t.userId)],
);

export const desktopTokens = sqliteTable(
  "desktop_tokens",
  {
    id: text("id").primaryKey(),
    desktopId: text("desktop_id")
      .notNull()
      .references(() => desktops.id, { onDelete: "cascade" }),
    /** SHA-256 of the 256-bit token, hex. */
    tokenHash: text("token_hash").notNull().unique(),
    /** Space separated subset of `relay push grants`. */
    scope: text("scope").notNull().default("relay push grants"),
    createdAt: integer("created_at").notNull(),
    lastUsedAt: integer("last_used_at"),
    revokedAt: integer("revoked_at"),
  },
  // Not in architecture.md 4.3: revoking a desktop updates all of its tokens.
  (t) => [index("desktop_tokens_desktop_id_idx").on(t.desktopId)],
);

export const devices = sqliteTable(
  "devices",
  {
    id: text("id").primaryKey(),
    userId: text("user_id")
      .notNull()
      .references(() => users.id, { onDelete: "cascade" }),
    name: text("name").notNull(),
    /** X25519, base64url. */
    publicKey: text("public_key").notNull(),
    fingerprint: text("fingerprint").notNull(),
    createdAt: integer("created_at").notNull(),
    /** Written when the device's last socket closes, never per heartbeat. */
    lastSeenAt: integer("last_seen_at"),
    revokedAt: integer("revoked_at"),
  },
  (t) => [index("devices_user_id_idx").on(t.userId)],
);

export const deviceGrants = sqliteTable(
  "device_grants",
  {
    deviceId: text("device_id")
      .notNull()
      .references(() => devices.id, { onDelete: "cascade" }),
    desktopId: text("desktop_id")
      .notNull()
      .references(() => desktops.id, { onDelete: "cascade" }),
    state: text("state", { enum: GRANT_STATES }).notNull().default("pending"),
    /** Applied by the desktop; null until it reports one. */
    permission: text("permission", { enum: PERMISSIONS }),
    requestedPermission: text("requested_permission", { enum: PERMISSIONS }),
    confirmedAt: integer("confirmed_at"),
    updatedAt: integer("updated_at").notNull(),
  },
  (t) => [
    primaryKey({ columns: [t.deviceId, t.desktopId] }),
    check("device_grants_state_check", oneOf("state", GRANT_STATES)),
    check("device_grants_permission_check", sql`${t.permission} IS NULL OR ${oneOf("permission", PERMISSIONS)}`),
    check(
      "device_grants_requested_permission_check",
      sql`${t.requestedPermission} IS NULL OR ${oneOf("requested_permission", PERMISSIONS)}`,
    ),
    index("device_grants_desktop_id_idx").on(t.desktopId),
    // Not in architecture.md 4.3: the cleanup of `pending` grants older than 7 days (WS-R fills it in).
    index("device_grants_state_updated_at_idx").on(t.state, t.updatedAt),
  ],
);

// --- Push ---

export const pushSubscriptions = sqliteTable(
  "push_subscriptions",
  {
    id: text("id").primaryKey(),
    userId: text("user_id")
      .notNull()
      .references(() => users.id, { onDelete: "cascade" }),
    deviceId: text("device_id")
      .notNull()
      .references(() => devices.id, { onDelete: "cascade" }),
    endpoint: text("endpoint").notNull().unique(),
    p256dh: text("p256dh").notNull(),
    auth: text("auth").notNull(),
    /** Which VAPID public key the subscription was created with, for rotation. */
    vapidKeyId: text("vapid_key_id").notNull(),
    ua: text("ua"),
    createdAt: integer("created_at").notNull(),
    lastSuccessAt: integer("last_success_at"),
    failures: integer("failures").notNull().default(0),
  },
  (t) => [
    index("push_subscriptions_user_id_idx").on(t.userId),
    // Not in architecture.md 4.3: the notify fan-out looks subscriptions up by device id.
    index("push_subscriptions_device_id_idx").on(t.deviceId),
  ],
);

export const pushPrefs = sqliteTable(
  "push_prefs",
  {
    userId: text("user_id")
      .notNull()
      .references(() => users.id, { onDelete: "cascade" }),
    desktopId: text("desktop_id")
      .notNull()
      .references(() => desktops.id, { onDelete: "cascade" }),
    enabled: bool("enabled").notNull().default(sql`0`),
    /** JSON: `{agentFinished, agentInput, monoMessages}` booleans. */
    categories: json<Record<string, boolean>>("categories").notNull().default(sql`'{}'`),
  },
  (t) => [primaryKey({ columns: [t.userId, t.desktopId] })],
);

// --- Pairing ---

export const pairingRequests = sqliteTable(
  "pairing_requests",
  {
    id: text("id").primaryKey(),
    /** 8 chars, ambiguity-free alphabet. */
    userCode: text("user_code").notNull().unique(),
    /** SHA-256 of the device code. Unique so `pair/poll` is an index search. */
    deviceCodeHash: text("device_code_hash").notNull().unique(),
    /** SHA-256 of the desktop's verifier. */
    challenge: text("challenge").notNull(),
    desktopPublicKey: text("desktop_public_key").notNull(),
    machineName: text("machine_name").notNull(),
    os: text("os"),
    confirmationCode: text("confirmation_code").notNull(),
    status: text("status", { enum: PAIRING_STATUSES }).notNull().default("pending"),
    userId: text("user_id").references(() => users.id, { onDelete: "set null" }),
    desktopId: text("desktop_id").references(() => desktops.id, { onDelete: "set null" }),
    createdAt: integer("created_at").notNull(),
    /** 10 minutes after creation. */
    expiresAt: integer("expires_at").notNull(),
    lastPolledAt: integer("last_polled_at"),
    /** Salted hash (AUDIT_IP_SALT). */
    ipHash: text("ip_hash"),
  },
  (t) => [
    check("pairing_requests_status_check", oneOf("status", PAIRING_STATUSES)),
    index("pairing_requests_expires_at_idx").on(t.expiresAt),
    // Not in architecture.md 4.3: "3 outstanding per IP" (security.md 3.5).
    index("pairing_requests_ip_hash_idx").on(t.ipHash),
  ],
);

// --- Instance state ---

export const settings = sqliteTable("settings", {
  key: text("key").primaryKey(),
  /** Row `invites_paused` is `0` or `1`; absent means `0`. Written only by `PUT /api/admin/settings`. */
  value: text("value").notNull(),
  updatedAt: integer("updated_at").notNull(),
  updatedBy: text("updated_by").references(() => users.id, { onDelete: "set null" }),
});

export const auditLog = sqliteTable(
  "audit_log",
  {
    id: text("id").primaryKey(),
    ts: integer("ts").notNull(),
    /** No FK: an account deletion leaves anonymized audit rows (security.md 10). */
    actorUserId: text("actor_user_id"),
    actorKind: text("actor_kind", { enum: AUDIT_ACTOR_KINDS }).notNull(),
    action: text("action").notNull(),
    targetType: text("target_type"),
    targetId: text("target_id"),
    /** JSON, no secrets and no content. The salted IP hash, when kept, lives here as `ipHash`. */
    meta: json<Record<string, unknown>>("meta").notNull().default(sql`'{}'`),
  },
  (t) => [
    check("audit_log_actor_kind_check", oneOf("actor_kind", AUDIT_ACTOR_KINDS)),
    // Retention cleanup and the admin view (newest first).
    index("audit_log_ts_idx").on(t.ts),
    // Not in architecture.md 4.3: `GET /api/audit` pages one user's events by the cursor (ts, id) (migration 0002).
    index("audit_log_actor_user_id_ts_id_idx").on(t.actorUserId, t.ts, t.id),
  ],
);

export const rateLimits = sqliteTable(
  "rate_limits",
  {
    key: text("key").notNull(),
    /** Start of the fixed window, unix seconds. */
    windowStart: integer("window_start").notNull(),
    count: integer("count").notNull().default(0),
  },
  (t) => [primaryKey({ columns: [t.key, t.windowStart] }), index("rate_limits_window_start_idx").on(t.windowStart)],
);

/** Every table, for tests and tooling. */
export const TABLES = {
  users,
  passkeys,
  challenges,
  recoveryCodes,
  sessions,
  invites,
  desktops,
  desktopTokens,
  devices,
  deviceGrants,
  pushSubscriptions,
  pushPrefs,
  pairingRequests,
  settings,
  auditLog,
  rateLimits,
} as const;

