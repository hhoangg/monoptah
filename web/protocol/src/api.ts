// Worker `/api/*` and `/relay/*` contracts (architecture.md 4.2, security.md 3-4):
// zod request and response schemas, inferred types, and a route registry. The
// Worker mounts one Hono sub-app per `module` from `API_ROUTES`; the web client
// builds its calls from the same entries. A test parses the 4.2 table in
// architecture.md and fails when this registry and the table diverge.
//
// JSON is camelCase. Times are unix seconds unless a name says otherwise. A failed
// call answers `ApiErrorSchema` (`{error, code}`, errors.ts). Secrets (tokens,
// recovery codes, invite codes) appear in a response only where the comment says so.
import { z } from "zod";

// --- Shared ---

const Id = z.string().min(1);
const Ok = z.object({ ok: z.literal(true) });
const Handle = z.string().regex(/^[a-z0-9_-]{3,32}$/, "3-32 chars, lowercase letters, digits, - and _");
const DisplayName = z.string().trim().min(1).max(64);
const Permission = z.enum(["read", "control"]);
const GrantState = z.enum(["pending", "confirmed", "revoked"]);
const Base64Url = z.string().min(1);

/** WebAuthn JSON from `@simplewebauthn`: kept open, the Worker library validates it. */
const WebAuthnOptions = z.looseObject({ challenge: z.string() });
const WebAuthnResponse = z.looseObject({ id: z.string(), type: z.literal("public-key") });

export const UserSchema = z.object({
  id: Id,
  handle: Handle,
  displayName: z.string(),
  role: z.enum(["user", "admin"]),
});
export type User = z.infer<typeof UserSchema>;

const WebAuthnChallenge = z.object({ challengeId: Id, options: WebAuthnOptions });
const WebAuthnVerifyBody = z.object({ challengeId: Id, response: WebAuthnResponse });

// --- Health ---

export const HealthResponse = z.object({ ok: z.literal(true) });

// --- Auth ---

export const RegisterOptionsBody = z.object({
  /** The invite code from the registration link fragment. */
  invite: z.string().min(1),
  handle: Handle,
  displayName: DisplayName,
});
export const RegisterOptionsResponse = WebAuthnChallenge;
export const RegisterVerifyBody = WebAuthnVerifyBody;
/** `recoveryCodes` (10) are returned once. The session cookie is set by the response. */
export const RegisterVerifyResponse = z.object({ user: UserSchema, recoveryCodes: z.array(z.string()).length(10) });

export const LoginOptionsBody = z.object({}).optional();
export const LoginOptionsResponse = WebAuthnChallenge;
export const LoginVerifyBody = WebAuthnVerifyBody;
export const LoginVerifyResponse = z.object({ user: UserSchema });

export const ReauthOptionsResponse = WebAuthnChallenge;
export const ReauthVerifyBody = WebAuthnVerifyBody;
/** `reauthAt` starts the 5 minute "fresh" window. */
export const ReauthVerifyResponse = z.object({ ok: z.literal(true), reauthAt: z.number() });

export const RecoverBody = z.object({ handle: Handle, code: z.string().min(1) });
/** The session is `recoveryOnly` (adds a passkey only). `codesLeft` triggers the regenerate prompt at 2 or fewer. */
export const RecoverResponse = z.object({ user: UserSchema, recoveryOnly: z.literal(true), codesLeft: z.number().int().nonnegative() });

// --- Me ---

export const MeResponse = z.object({
  user: UserSchema,
  invitesLeft: z.number().int().nonnegative(),
  recoveryOnly: z.boolean(),
  /** null forces a recovery-code regenerate at sign-in. */
  recoveryAckAt: z.number().nullable(),
});
export const PatchMeBody = z.object({ displayName: DisplayName });
export const PatchMeResponse = z.object({ user: UserSchema });

// --- Audit (own events, paged) ---

export const AuditEventSchema = z.object({
  id: Id,
  ts: z.number(),
  action: z.string(),
  targetType: z.string().nullable(),
  targetId: z.string().nullable(),
  /** No secrets, no content. */
  meta: z.record(z.string(), z.unknown()),
});
export const PageQuery = z.object({ cursor: z.string().optional(), limit: z.coerce.number().int().min(1).max(100).optional() });
export const AuditResponse = z.object({ events: z.array(AuditEventSchema), nextCursor: z.string().nullable() });

// --- Sessions (cookie sessions) ---

export const SessionInfoSchema = z.object({
  id: Id,
  createdAt: z.number(),
  lastSeenAt: z.number(),
  userAgent: z.string().nullable(),
  amr: z.enum(["passkey", "recovery"]),
  current: z.boolean(),
});
export const SessionsResponse = z.object({ sessions: z.array(SessionInfoSchema) });

// --- Passkeys ---

export const PasskeySchema = z.object({
  id: Id,
  name: z.string(),
  createdAt: z.number(),
  lastUsedAt: z.number().nullable(),
  deviceType: z.string().nullable(),
  backedUp: z.boolean(),
  transports: z.array(z.string()),
});
export const PasskeysResponse = z.object({ passkeys: z.array(PasskeySchema) });
export const AddPasskeyOptionsResponse = WebAuthnChallenge;
export const AddPasskeyVerifyBody = WebAuthnVerifyBody.extend({ name: z.string().trim().min(1).max(64).optional() });
export const PasskeyResponse = z.object({ passkey: PasskeySchema });
export const RenamePasskeyBody = z.object({ name: z.string().trim().min(1).max(64) });

// --- Recovery codes ---

/** The 10 new codes are returned once; all older codes stop working. */
export const RegenerateRecoveryResponse = z.object({ recoveryCodes: z.array(z.string()).length(10) });

// --- Invites ---

export const InviteSchema = z.object({
  id: Id,
  createdAt: z.number(),
  expiresAt: z.number(),
  usedAt: z.number().nullable(),
  revokedAt: z.number().nullable(),
  usedBy: z.string().nullable(),
});
export const InvitesResponse = z.object({ invites: z.array(InviteSchema), granted: z.number().int(), remaining: z.number().int() });
/** `code` is shown once to its creator. Fails with `code: invites_paused` while paused or at MAX_USERS. */
export const CreateInviteResponse = z.object({ invite: InviteSchema, code: z.string() });

// --- Pairing (desktop device authorization, architecture.md 6.4) ---

export const PairStartBody = z.object({
  /** Desktop X25519 public key, base64url. */
  publicKey: Base64Url,
  machineName: z.string().trim().min(1).max(64),
  os: z.string().max(32),
  appVersion: z.string().max(32),
  /** SHA-256 of the desktop's 32 random verifier bytes, base64url. */
  challenge: Base64Url,
});
export const PairStartResponse = z.object({
  userCode: z.string(),
  deviceCode: z.string(),
  confirmationCode: z.string(),
  expiresIn: z.number().int(),
  interval: z.number().int(),
});

export const PairPollBody = z.object({ deviceCode: z.string().min(1), verifier: Base64Url });
export const PairPollResponse = z.discriminatedUnion("status", [
  z.object({ status: z.literal("pending") }),
  z.object({ status: z.literal("slow_down") }),
  z.object({ status: z.literal("denied") }),
  z.object({ status: z.literal("expired") }),
  /** Returned exactly once; the row becomes `consumed`. */
  z.object({
    status: z.literal("approved"),
    desktopId: Id,
    desktopToken: z.string(),
    account: z.object({ handle: Handle }),
    relayUrl: z.string(),
  }),
]);
export type PairPollResponse = z.infer<typeof PairPollResponse>;

export const PairInfoResponse = z.object({
  machineName: z.string(),
  os: z.string(),
  /** Full lowercase hex SHA-256 of the desktop public key. */
  fingerprint: z.string(),
  confirmationCode: z.string(),
  expiresAt: z.number(),
});
/** `replaces`: re-pair an existing desktop (keeps id, name and color; architecture.md 6.7). */
export const PairApproveBody = z.object({ replaces: Id.optional() });
export const PairApproveResponse = z.object({ desktopId: Id, name: z.string() });

// --- Desktops ---

export const DesktopSummarySchema = z.object({
  id: Id,
  name: z.string(),
  color: z.string().nullable(),
  online: z.boolean(),
  /** Pending device confirmations. */
  pendingCount: z.number().int().nonnegative(),
  os: z.string().nullable(),
  appVersion: z.string().nullable(),
  lastSeenAt: z.number().nullable(),
});
export const DesktopsResponse = z.object({ desktops: z.array(DesktopSummarySchema) });
/** The directory entry the device pins (architecture.md 3.3 step 1). */
export const DesktopDirectoryResponse = DesktopSummarySchema.extend({
  /** X25519, base64url. */
  publicKey: Base64Url,
  /** Full lowercase hex SHA-256 of the public key. */
  fingerprint: z.string(),
});
export const PatchDesktopBody = z.object({ name: z.string().trim().min(1).max(64).optional(), color: z.string().max(32).nullable().optional() });
export const PatchDesktopResponse = z.object({ desktop: DesktopSummarySchema });

// --- Devices (browsers) ---

export const DeviceSchema = z.object({
  id: Id,
  name: z.string(),
  fingerprint: z.string(),
  createdAt: z.number(),
  lastSeenAt: z.number().nullable(),
});
export const RegisterDeviceBody = z.object({ name: z.string().trim().min(1).max(64), publicKey: Base64Url });
export const RegisterDeviceResponse = z.object({ deviceId: Id });
export const DevicesResponse = z.object({ devices: z.array(DeviceSchema) });
export const PatchDeviceBody = z.object({ name: z.string().trim().min(1).max(64) });
export const PatchDeviceResponse = z.object({ device: DeviceSchema });

// --- Grants ---

export const GrantSchema = z.object({
  deviceId: Id,
  deviceName: z.string(),
  fingerprint: z.string(),
  state: GrantState,
  /** Applied by the desktop. */
  permission: Permission.nullable(),
  requestedPermission: Permission.nullable(),
  confirmedAt: z.number().nullable(),
  updatedAt: z.number(),
});
export const RequestGrantBody = z.object({ deviceId: Id });
export const GrantResponse = z.object({ grant: GrantSchema });
export const GrantsResponse = z.object({ grants: z.array(GrantSchema) });
/** `requestedPermission` asks for a change; `state: "revoked"` revokes (fresh re-auth). */
export const PutGrantBody = z.object({ requestedPermission: Permission.optional(), state: z.literal("revoked").optional() });

// --- Desktop token routes (/api/desktop/*) ---

export const DesktopMeResponse = z.object({ desktopId: Id, name: z.string(), account: z.object({ handle: Handle }) });
export const DesktopGrantSchema = z.object({
  deviceId: Id,
  /** Device X25519 public key, base64url. */
  publicKey: Base64Url,
  fingerprint: z.string(),
  state: GrantState,
  requestedPermission: Permission.nullable(),
  permission: Permission.nullable(),
});
export const DesktopGrantsResponse = z.object({ grants: z.array(DesktopGrantSchema) });
/** The desktop reports what it applied. `revoked` calls `room.revoke` for the device. */
export const DesktopGrantStateBody = z.object({ state: GrantState, permission: Permission.optional() });

// --- Push ---

export const PUSH_CATEGORIES = ["agentFinished", "agentInput", "monoMessages"] as const;
export const PushCategory = z.enum(PUSH_CATEGORIES);
export const VapidKeyResponse = z.object({ publicKey: Base64Url, keyId: z.string() });
export const PushSubscriptionBody = z.object({
  endpoint: z.url(),
  keys: z.object({ p256dh: Base64Url, auth: Base64Url }),
  deviceId: Id,
  vapidKeyId: z.string(),
});
export const PushSubscriptionResponse = z.object({ id: Id });
export const PushPrefsBody = z.object({ enabled: z.boolean(), categories: z.record(PushCategory, z.boolean()) });
export const PushPrefsResponse = z.object({ enabled: z.boolean(), categories: z.record(PushCategory, z.boolean()) });

/** Desktop -> Worker (architecture.md 8). No title, no assistant text, no transcript. */
export const PushNotifyBody = z.object({
  category: PushCategory,
  /** Display name of the project (the Mono name for Mono notices). */
  project: z.string(),
  status: z.enum(["finished", "needs_input", "message"]),
  sessionId: z.string(),
  monoId: z.string().optional(),
  habitRun: z.boolean().optional(),
  /** Server ids of confirmed web devices whose selection includes the project or Mono. */
  deviceIds: z.array(Id),
  desktopName: z.string().optional(),
});
export const PushNotifyResponse = z.object({ sent: z.number().int().nonnegative() });

/** The encrypted payload the service worker receives (architecture.md 8, under 1 KiB). */
export const PushPayloadSchema = z.object({
  v: z.literal(1),
  /** Desktop name. */
  t: z.string(),
  /** `<project>: Finished`, `: Needs your input` or `: New message`. */
  b: z.string(),
  url: z.string(),
  /** The session id; repeated notices for one session collapse. */
  tag: z.string(),
});
export type PushPayload = z.infer<typeof PushPayloadSchema>;

// --- Admin (security.md 4) ---

export const AdminUserSchema = z.object({
  id: Id,
  handle: Handle,
  displayName: z.string(),
  role: z.enum(["user", "admin"]),
  status: z.enum(["active", "blocked"]),
  invitedBy: z.string().nullable(),
  blockedVia: z.string().nullable(),
  invitesGranted: z.number().int(),
  desktopCount: z.number().int(),
  createdAt: z.number(),
});
export const AdminUsersResponse = z.object({ users: z.array(AdminUserSchema) });
export const AdminBlockBody = z.object({ cascade: z.boolean(), reason: z.string().max(500) });
/** `blocked` counts the users affected (more than 1 for a branch). */
export const AdminBlockResponse = z.object({ blocked: z.number().int().nonnegative() });
export const AdminGrantInvitesBody = z.object({ count: z.number().int().min(1).max(100) });
export const AdminSettingsSchema = z.object({ invitesPaused: z.boolean(), maxUsers: z.number().int().nonnegative(), userCount: z.number().int().nonnegative() });
export const AdminPutSettingsBody = z.object({ invitesPaused: z.boolean() });
export const AdminAuditResponse = z.object({ events: z.array(AuditEventSchema.extend({ actorUserId: z.string().nullable(), actorKind: z.enum(["user", "desktop", "admin", "system"]) })), nextCursor: z.string().nullable() });

// --- Registry ---

export const API_METHODS = ["GET", "POST", "PUT", "PATCH", "DELETE"] as const;
export type ApiMethod = (typeof API_METHODS)[number];

/**
 * - `public`, `session`: as in architecture.md 4.2.
 * - `fresh`: session plus a passkey re-auth within 5 minutes.
 * - `token:<scope>`: desktop bearer token with that scope.
 * - `admin`: session, role admin, fresh.
 * - `session+grant`: session plus a device grant (relay device upgrade).
 */
export const API_AUTH = ["public", "session", "fresh", "token:relay", "token:push", "token:grants", "admin", "session+grant"] as const;
export type ApiAuth = (typeof API_AUTH)[number];

/** One Hono sub-app per module. `ratelimit` has no routes of its own (middleware and `rate_limits` table). */
export const API_MODULES = [
  "health",
  "auth",
  "me",
  "audit",
  "sessions",
  "passkeys",
  "recovery",
  "invites",
  "admin",
  "pairing",
  "desktops",
  "devices",
  "grants",
  "push",
  "relay",
] as const;
export type ApiModule = (typeof API_MODULES)[number];

export interface RouteRequest {
  /** Path parameters (`:id` and so on). Values are strings on the wire. */
  params?: z.ZodType;
  query?: z.ZodType;
  body?: z.ZodType;
}

/**
 * Rate limit groups of security.md 3.5, named after its table rows. A route without a group is
 * either unlimited (health, public GETs) or falls under the general authenticated limit
 * (120 per session per minute, in memory), which needs no per-route flag.
 */
export const RATE_LIMIT_GROUPS = ["register", "login", "recover", "pair-start", "pair-poll", "push-notify"] as const;
export type RateLimitGroup = (typeof RATE_LIMIT_GROUPS)[number];

export interface ApiRoute {
  method: ApiMethod;
  /** Hono style, for example `/api/desktops/:id`. */
  path: string;
  auth: ApiAuth;
  module: ApiModule;
  request: RouteRequest;
  /** Success body. A WebSocket upgrade route has `upgrade: true` and `response: z.null()`. */
  response: z.ZodType;
  /** The security.md 3.5 group whose D1 window limits this route. */
  rateLimit?: RateLimitGroup;
  /** `recovery_only` sessions may call it (add a passkey). */
  allowRecoveryOnly?: boolean;
  /** Needs a fresh re-auth only for some requests (for example revoking a grant). */
  freshWhen?: string;
  /** Owner only: the resource must belong to the session user. */
  owner?: boolean;
  /** WebSocket upgrade, not JSON. */
  upgrade?: boolean;
}

const idParam = z.object({ id: Id });
const userCodeParam = z.object({ userCode: z.string().min(1) });
const desktopDeviceParams = z.object({ id: Id, deviceId: Id });
const deviceIdParam = z.object({ deviceId: Id });

function r(route: ApiRoute): ApiRoute {
  return route;
}

/** Every row of architecture.md 4.2. Rows with two routes in one cell count as two. */
export const API_ROUTES: readonly ApiRoute[] = [
  r({ method: "GET", path: "/api/health", auth: "public", module: "health", request: {}, response: HealthResponse }),

  r({ method: "POST", path: "/api/auth/register/options", rateLimit: "register", auth: "public", module: "auth", request: { body: RegisterOptionsBody }, response: RegisterOptionsResponse }),
  r({ method: "POST", path: "/api/auth/register/verify", rateLimit: "register", auth: "public", module: "auth", request: { body: RegisterVerifyBody }, response: RegisterVerifyResponse }),
  r({ method: "POST", path: "/api/auth/login/options", rateLimit: "login", auth: "public", module: "auth", request: { body: LoginOptionsBody }, response: LoginOptionsResponse }),
  r({ method: "POST", path: "/api/auth/login/verify", rateLimit: "login", auth: "public", module: "auth", request: { body: LoginVerifyBody }, response: LoginVerifyResponse }),
  r({ method: "POST", path: "/api/auth/reauth/options", auth: "session", module: "auth", request: {}, response: ReauthOptionsResponse }),
  r({ method: "POST", path: "/api/auth/reauth/verify", auth: "session", module: "auth", request: { body: ReauthVerifyBody }, response: ReauthVerifyResponse }),
  r({ method: "POST", path: "/api/auth/recover", rateLimit: "recover", auth: "public", module: "auth", request: { body: RecoverBody }, response: RecoverResponse }),
  r({ method: "POST", path: "/api/auth/logout", auth: "session", module: "auth", request: {}, response: Ok }),

  r({ method: "GET", path: "/api/me", auth: "session", module: "me", request: {}, response: MeResponse }),
  r({ method: "PATCH", path: "/api/me", auth: "session", module: "me", request: { body: PatchMeBody }, response: PatchMeResponse }),
  r({ method: "DELETE", path: "/api/me", auth: "fresh", module: "me", request: {}, response: Ok }),

  r({ method: "GET", path: "/api/audit", auth: "session", module: "audit", request: { query: PageQuery }, response: AuditResponse }),

  r({ method: "GET", path: "/api/sessions", auth: "session", module: "sessions", request: {}, response: SessionsResponse }),
  r({ method: "DELETE", path: "/api/sessions/:id", auth: "session", module: "sessions", request: { params: idParam }, response: Ok }),

  r({ method: "GET", path: "/api/passkeys", auth: "session", module: "passkeys", request: {}, response: PasskeysResponse }),
  r({ method: "POST", path: "/api/passkeys/options", auth: "fresh", module: "passkeys", allowRecoveryOnly: true, request: {}, response: AddPasskeyOptionsResponse }),
  r({ method: "POST", path: "/api/passkeys/verify", auth: "fresh", module: "passkeys", allowRecoveryOnly: true, request: { body: AddPasskeyVerifyBody }, response: PasskeyResponse }),
  r({ method: "PATCH", path: "/api/passkeys/:id", auth: "fresh", module: "passkeys", request: { params: idParam, body: RenamePasskeyBody }, response: PasskeyResponse }),
  r({ method: "DELETE", path: "/api/passkeys/:id", auth: "fresh", module: "passkeys", request: { params: idParam }, response: Ok }),

  r({ method: "POST", path: "/api/recovery-codes/regenerate", auth: "fresh", module: "recovery", request: {}, response: RegenerateRecoveryResponse }),

  r({ method: "GET", path: "/api/invites", auth: "session", module: "invites", request: {}, response: InvitesResponse }),
  r({ method: "POST", path: "/api/invites", auth: "session", module: "invites", request: {}, response: CreateInviteResponse }),
  r({ method: "DELETE", path: "/api/invites/:id", auth: "session", module: "invites", request: { params: idParam }, response: Ok }),

  r({ method: "POST", path: "/api/pair/start", rateLimit: "pair-start", auth: "public", module: "pairing", request: { body: PairStartBody }, response: PairStartResponse }),
  r({ method: "POST", path: "/api/pair/poll", rateLimit: "pair-poll", auth: "public", module: "pairing", request: { body: PairPollBody }, response: PairPollResponse }),
  r({ method: "GET", path: "/api/pair/:userCode", auth: "session", module: "pairing", request: { params: userCodeParam }, response: PairInfoResponse }),
  r({ method: "POST", path: "/api/pair/:userCode/approve", auth: "fresh", module: "pairing", request: { params: userCodeParam, body: PairApproveBody }, response: PairApproveResponse }),
  r({ method: "POST", path: "/api/pair/:userCode/deny", auth: "session", module: "pairing", request: { params: userCodeParam }, response: Ok }),

  r({ method: "GET", path: "/api/desktops", auth: "session", module: "desktops", request: {}, response: DesktopsResponse }),
  r({ method: "GET", path: "/api/desktops/:id", auth: "session", module: "desktops", owner: true, request: { params: idParam }, response: DesktopDirectoryResponse }),
  r({ method: "PATCH", path: "/api/desktops/:id", auth: "session", module: "desktops", owner: true, request: { params: idParam, body: PatchDesktopBody }, response: PatchDesktopResponse }),
  r({ method: "DELETE", path: "/api/desktops/:id", auth: "fresh", module: "desktops", owner: true, request: { params: idParam }, response: Ok }),

  r({ method: "POST", path: "/api/devices", auth: "session", module: "devices", request: { body: RegisterDeviceBody }, response: RegisterDeviceResponse }),
  r({ method: "GET", path: "/api/devices", auth: "session", module: "devices", request: {}, response: DevicesResponse }),
  r({ method: "PATCH", path: "/api/devices/:id", auth: "session", module: "devices", request: { params: idParam, body: PatchDeviceBody }, response: PatchDeviceResponse }),
  r({ method: "DELETE", path: "/api/devices/:id", auth: "session", module: "devices", request: { params: idParam }, response: Ok }),

  r({ method: "POST", path: "/api/desktops/:id/grants", auth: "session", module: "grants", owner: true, request: { params: idParam, body: RequestGrantBody }, response: GrantResponse }),
  r({ method: "GET", path: "/api/desktops/:id/grants", auth: "session", module: "grants", owner: true, request: { params: idParam }, response: GrantsResponse }),
  r({ method: "PUT", path: "/api/desktops/:id/grants/:deviceId", auth: "session", module: "grants", owner: true, freshWhen: 'body.state === "revoked"', request: { params: desktopDeviceParams, body: PutGrantBody }, response: GrantResponse }),

  r({ method: "GET", path: "/api/desktop/me", auth: "token:relay", module: "desktops", request: {}, response: DesktopMeResponse }),
  r({ method: "DELETE", path: "/api/desktop/me", auth: "token:relay", module: "desktops", request: {}, response: Ok }),
  r({ method: "GET", path: "/api/desktop/grants", auth: "token:grants", module: "grants", request: {}, response: DesktopGrantsResponse }),
  r({ method: "POST", path: "/api/desktop/grants/:deviceId/state", auth: "token:grants", module: "grants", request: { params: deviceIdParam, body: DesktopGrantStateBody }, response: Ok }),

  r({ method: "GET", path: "/api/push/vapid-key", auth: "public", module: "push", request: {}, response: VapidKeyResponse }),
  r({ method: "POST", path: "/api/push/subscriptions", auth: "session", module: "push", request: { body: PushSubscriptionBody }, response: PushSubscriptionResponse }),
  r({ method: "DELETE", path: "/api/push/subscriptions/:id", auth: "session", module: "push", request: { params: idParam }, response: Ok }),
  r({ method: "PUT", path: "/api/push/prefs/:desktopId", auth: "session", module: "push", owner: true, request: { params: z.object({ desktopId: Id }), body: PushPrefsBody }, response: PushPrefsResponse }),
  r({ method: "POST", path: "/api/push/notify", rateLimit: "push-notify", auth: "token:push", module: "push", request: { body: PushNotifyBody }, response: PushNotifyResponse }),

  r({ method: "GET", path: "/api/admin/users", auth: "admin", module: "admin", request: {}, response: AdminUsersResponse }),
  r({ method: "POST", path: "/api/admin/users/:id/block", auth: "admin", module: "admin", request: { params: idParam, body: AdminBlockBody }, response: AdminBlockResponse }),
  r({ method: "POST", path: "/api/admin/users/:id/unblock", auth: "admin", module: "admin", request: { params: idParam }, response: Ok }),
  r({ method: "POST", path: "/api/admin/users/:id/invites", auth: "admin", module: "admin", request: { params: idParam, body: AdminGrantInvitesBody }, response: Ok }),
  r({ method: "GET", path: "/api/admin/settings", auth: "admin", module: "admin", request: {}, response: AdminSettingsSchema }),
  r({ method: "PUT", path: "/api/admin/settings", auth: "admin", module: "admin", request: { body: AdminPutSettingsBody }, response: AdminSettingsSchema }),
  r({ method: "GET", path: "/api/admin/audit", auth: "admin", module: "admin", request: { query: PageQuery }, response: AdminAuditResponse }),

  r({ method: "GET", path: "/relay/desktop/:desktopId", auth: "token:relay", module: "relay", upgrade: true, request: { params: z.object({ desktopId: Id }) }, response: z.null() }),
  r({ method: "GET", path: "/relay/device/:desktopId", auth: "session+grant", module: "relay", upgrade: true, request: { params: z.object({ desktopId: Id }), query: z.object({ device: Id }) }, response: z.null() }),
];

export function findRoute(method: string, path: string): ApiRoute | undefined {
  return API_ROUTES.find((route) => route.method === method && route.path === path);
}

/** Routes of one module, for `app.route("/", module)` style mounting. */
export function routesOfModule(module: ApiModule): ApiRoute[] {
  return API_ROUTES.filter((route) => route.module === module);
}
