// Registry of audit events (security.md 10). Every event the Worker writes is listed here with the strict
// schema of its `meta`; `audit.ts` refuses anything else. `meta` holds ids, counts, booleans and short enums
// only: never a secret, a code, a token, a challenge, a handle or any user content. The salted IP hash, when
// kept, is `meta.ipHash`.
import { z } from "zod";
import type { AUDIT_ACTOR_KINDS } from "../../db/schema.ts";

export type AuditActorKind = (typeof AUDIT_ACTOR_KINDS)[number];

/**
 * Meta keys that must never exist in any schema, whatever their type: they name secrets or content.
 * Matched case-insensitively on the whole key (`code` is forbidden, `codesLeft` is a count and fine).
 * A schema that contains one throws at module load (`assertSafeMetaKeys`), and a test checks the registry.
 */
export const FORBIDDEN_META_KEYS: readonly string[] = [
  "code",
  "token",
  "secret",
  "cookie",
  "pepper",
  "challenge",
  "password",
  "body",
  "content",
  "handle",
  "displayname",
  "name",
  "email",
  "ip",
  "authorization",
  "signature",
  "credential",
  "publickey",
  "key",
  "text",
  "message",
  "payload",
];

/** Throws when a meta schema has a forbidden key, so a schema that could carry a secret cannot be registered. */
export function assertSafeMetaKeys(action: string, schema: z.ZodObject): void {
  for (const key of Object.keys(schema.shape)) {
    if (FORBIDDEN_META_KEYS.includes(key.toLowerCase())) {
      throw new Error(`Audit action ${action}: meta key "${key}" is forbidden (secret or content)`);
    }
  }
}

// --- Field types: only ids, counts, booleans and short enums ---

/** An opaque id (UUID, credential id, derived session id): url-safe characters only, at most 128 chars. */
const Id = z.string().min(1).max(128).regex(/^[A-Za-z0-9_.-]+$/);
/**
 * A passkey credential id: base64url, up to 1023 bytes (WebAuthn) which is 1364 characters. Authenticators choose
 * the length (security keys use 64 bytes or more), so `Id` (128) would make login and add-passkey throw. The id is
 * stored as is; the registration refuses anything above 1023 bytes (`MAX_CREDENTIAL_ID_BYTES`).
 */
const PasskeyId = z.string().min(1).max(1400).regex(/^[A-Za-z0-9_-]+$/);
const Count = z.number().int().min(0).max(1_000_000);
const PushCategory = z.enum(["agentFinished", "agentInput", "monoMessages"]);
const IpHash = z.string().regex(/^[0-9a-f]{64}$/);
const Permission = z.enum(["read", "control"]);
const GrantState = z.enum(["pending", "confirmed", "revoked"]);
/** A release label such as `1.4.0` or `1.4.0+x`; the same pattern as `scripts/deploy-official.mjs`. */
const Version = z.string().regex(/^[0-9A-Za-z.+-]{1,32}$/);
/** A full commit sha, lowercase hex. */
const Commit = z.string().regex(/^[0-9a-f]{40}$/);
/** The rate limit groups of security.md 3.5, plus the ones that need data only a handler has. */
export const RATE_LIMIT_AUDIT_GROUPS = [
  "register",
  "login",
  "recover",
  "pair-start",
  "pair-poll",
  "push-notify",
  "push-test",
  "terminal-ticket",
  "invite",
  "handle",
  "session",
  "session-avatar",
] as const;

interface ActionSpec {
  /** Who may be the actor of this event. The first entry is the default. */
  actorKinds: readonly [AuditActorKind, ...AuditActorKind[]];
  meta: z.ZodObject;
}

function spec<S extends z.ZodRawShape>(actorKinds: ActionSpec["actorKinds"], shape: S): { actorKinds: ActionSpec["actorKinds"]; meta: z.ZodObject<S, z.core.$strict> } {
  return { actorKinds, meta: z.strictObject(shape) };
}

/** What `target_type` and `target_id` of a row refer to is free for the writer, but the action fixes the meta. */
export const AUDIT_ACTIONS = {
  // Accounts
  "account.register": spec(["user"], { inviteId: Id.optional(), ipHash: IpHash.optional() }),
  "account.login": spec(["user"], { passkeyId: PasskeyId, ipHash: IpHash.optional() }),
  /** A signature counter that did not increase: the login is refused (security.md 3.1). */
  "account.login_refused_counter": spec(["system", "user"], { passkeyId: PasskeyId, ipHash: IpHash.optional() }),
  // Passkeys
  "passkey.add": spec(["user"], { passkeyId: PasskeyId, count: Count.optional() }),
  "passkey.rename": spec(["user"], { passkeyId: PasskeyId }),
  "passkey.delete": spec(["user"], { passkeyId: PasskeyId, remaining: Count }),
  // Recovery
  "recovery.use": spec(["user"], { codesLeft: Count, revokedSessions: Count.optional(), ipHash: IpHash.optional() }),
  "recovery.regenerate": spec(["user"], { batch: Count }),
  // Invites
  "invite.create": spec(["user", "admin"], { inviteId: Id }),
  "invite.use": spec(["user", "system"], { inviteId: Id }),
  "invite.revoke": spec(["user", "admin", "system"], { inviteId: Id, count: Count.optional() }),
  // Pairing, desktops and devices
  "pairing.approve": spec(["user"], { desktopId: Id, replaced: z.boolean().optional() }),
  "desktop.create": spec(["user"], { desktopId: Id }),
  "desktop.revoke": spec(["user", "desktop"], { desktopId: Id }),
  "device.create": spec(["user"], { deviceId: Id }),
  "device.revoke": spec(["user"], { deviceId: Id, grants: Count.optional() }),
  "grant.change": spec(["user", "desktop"], {
    deviceId: Id,
    desktopId: Id,
    state: GrantState.optional(),
    permission: Permission.optional(),
    requestedPermission: Permission.optional(),
  }),
  /** A web device got a single-use ticket to widen what it may see on a desktop (fresh passkey re-auth). */
  "selection.ticket": spec(["user"], { deviceId: Id, desktopId: Id }),
  /** The desktop consumed such a ticket: the widening is going ahead. */
  "selection.widen": spec(["desktop"], { deviceId: Id, desktopId: Id }),
  /** A web device got a single-use ticket to open a terminal on a desktop (fresh passkey re-auth, bound to one project). */
  "terminal.ticket": spec(["user"], { deviceId: Id, desktopId: Id }),
  /** The desktop consumed such a ticket: the terminal is about to open. */
  "terminal.ticket.consumed": spec(["desktop"], { deviceId: Id, desktopId: Id }),
  // Admin
  "user.block": spec(["admin"], { cascade: z.boolean(), count: Count }),
  "user.unblock": spec(["admin"], { cascade: z.boolean().optional() }),
  "admin.settings_change": spec(["admin"], { invitesPaused: z.boolean() }),
  "admin.invites_grant": spec(["admin"], { count: Count }),
  // Sessions
  "session.logout": spec(["user"], {}),
  "session.revoke": spec(["user", "admin", "system"], { all: z.boolean(), count: Count }),
  // Push delivery (metadata only: ids and counts)
  /** One row per notify attempt that passed the prefs and dedupe checks and reached the recipient stage. */
  "push.notify": spec(["desktop"], { desktopId: Id, category: PushCategory, recipients: Count, sent: Count, gone: Count, failed: Count, backoff: Count }),
  /** `POST /api/push/test`: a test notification to one of the caller's own devices. */
  "push.test": spec(["user"], { deviceId: Id, recipients: Count, sent: Count, gone: Count, failed: Count, backoff: Count }),
  // Abuse and operations
  /** Written once per key per window when the count first exceeds the limit (modules/ratelimit). */
  "ratelimit.breach": spec(["system", "user"], {
    group: z.enum(RATE_LIMIT_AUDIT_GROUPS),
    limit: Count,
    windowSeconds: Count,
    ipHash: IpHash.optional(),
  }),
  /** Written by `scripts/deploy-official.mjs`, which validates both values against the same two patterns. */
  "system.deploy": spec(["system"], {
    version: Version,
    commit: Commit,
  }),
} as const satisfies Record<string, ActionSpec>;

export type AuditAction = keyof typeof AUDIT_ACTIONS;

/** Every registered action name. */
export const AUDIT_ACTION_NAMES = Object.keys(AUDIT_ACTIONS) as AuditAction[];

/** The meta a writer passes for action `A`. */
export type AuditMeta<A extends AuditAction> = z.input<(typeof AUDIT_ACTIONS)[A]["meta"]>;

for (const [action, entry] of Object.entries(AUDIT_ACTIONS)) assertSafeMetaKeys(action, entry.meta);
