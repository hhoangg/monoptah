// Dev-only client-side stand-in for the Worker `/api/*` routes WS-S calls (A-03..A-08 contracts).
// It does not verify WebAuthn signatures: any well-formed response is accepted. The Worker rules it
// does imitate: Origin-free CSRF and content-type checks, session and fresh gates, `recovery_only`
// confinement, invite quota and pause, the last-passkey rule, the generic invalid-invite error and the
// pairing approval routes (`GET /api/pair/:userCode`, `approve` with `replaces`, `deny`).
import { findRoute, USER_CODE_ALPHABET, USER_CODE_LENGTH, type ApiErrorCode } from "@monoptah/protocol";
import type { ApiTransport } from "../api";
import {
  base64url,
  createMockState,
  defaultPrefs,
  FRESH_WINDOW_SECONDS,
  MAX_DESKTOPS,
  newRecoveryBatch,
  normalizeUserCode,
  normalizeRecoveryCode,
  nowSeconds,
  PAIRING_TTL_SECONDS,
  randomBytes,
  randomId,
  userCount,
  invitesClosed,
  MAX_INVITES_GRANTED,
  type MockAdminUser,
  type MockAuditEvent,
  type MockChallenge,
  type MockPairing,
  type MockState,
  type SeedOptions,
} from "./state";

class MockHttpError extends Error {
  readonly status: number;
  readonly code: ApiErrorCode;
  readonly headers: Record<string, string>;

  constructor(status: number, code: ApiErrorCode, message: string, headers: Record<string, string> = {}) {
    super(message);
    this.status = status;
    this.code = code;
    this.headers = headers;
  }
}

interface Ctx {
  state: MockState;
  params: Record<string, string>;
  body: Record<string, unknown>;
  query: URLSearchParams;
}

type Handler = (ctx: Ctx) => unknown;

const CHALLENGE_TTL_SECONDS = 300;
const INVITE_TTL_SECONDS = 7 * 86400;
const MAX_RECOVER_FAILURES = 5;
const RECOVERY_ONLY_ALLOWLIST: ReadonlySet<string> = new Set(["GET /api/me", "POST /api/auth/logout"]);
/** A well-formed 65 byte uncompressed P-256 point (first byte 4), base64url, for `applicationServerKey`. */
export const MOCK_VAPID_PUBLIC_KEY = "BAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA";
export const MOCK_VAPID_KEY_ID = "mock-vapid-1";
const INVALID_INVITE = "That invite is not valid";

function expectString(value: unknown): string {
  return typeof value === "string" ? value : "";
}

// --- WebAuthn options (valid JSON for @simplewebauthn/browser) ---

function rpId(): string {
  return globalThis.location?.hostname || "localhost";
}

function issueChallenge(state: MockState, challenge: Omit<MockChallenge, "expiresAt">): { challengeId: string; challenge: string } {
  const challengeId = randomId("ch");
  state.challenges.set(challengeId, { ...challenge, expiresAt: nowSeconds() + CHALLENGE_TTL_SECONDS });
  return { challengeId, challenge: base64url(randomBytes(32)) };
}

function creationOptions(challenge: string, user: { handle: string; displayName: string }) {
  return {
    rp: { id: rpId(), name: "Monoptah" },
    user: { id: base64url(randomBytes(16)), name: user.handle, displayName: user.displayName },
    challenge,
    pubKeyCredParams: [
      { type: "public-key", alg: -7 },
      { type: "public-key", alg: -257 },
    ],
    timeout: 60000,
    attestation: "none",
    excludeCredentials: [],
    authenticatorSelection: { residentKey: "required", requireResidentKey: true, userVerification: "required" },
  };
}

function requestOptions(challenge: string) {
  return { challenge, rpId: rpId(), timeout: 60000, userVerification: "required" };
}

/** Single use, 5 minutes, bound to its purpose. */
function takeChallenge(state: MockState, id: unknown, purpose: MockChallenge["purpose"]): MockChallenge {
  const challenge = typeof id === "string" ? state.challenges.get(id) : undefined;
  if (typeof id === "string") state.challenges.delete(id);
  if (!challenge || challenge.purpose !== purpose) throw new MockHttpError(400, "invalid", "Unknown or reused challenge");
  if (challenge.expiresAt < nowSeconds()) throw new MockHttpError(400, "invalid", "Challenge expired or already used");
  return challenge;
}

// --- Helpers ---

function invitesRemaining(state: MockState): number {
  return Math.max(0, state.invitesGranted - state.invites.filter((invite) => invite.revokedAt === null).length);
}

function meBody(state: MockState) {
  return {
    user: state.user,
    invitesLeft: invitesRemaining(state),
    recoveryOnly: state.recoveryOnly,
    recoveryCodesLeft: state.recoveryCodes.size,
    recoveryAckAt: state.recoveryAckAt,
  };
}

function startSession(state: MockState, amr: "passkey" | "recovery"): void {
  const now = nowSeconds();
  state.signedIn = true;
  state.recoveryOnly = amr === "recovery";
  state.reauthAt = null;
  state.sessions = [
    { id: "sess-current", createdAt: now, lastSeenAt: now, userAgent: "Fake Browser/1.0 (current)", amr, current: true },
  ];
}

function isFresh(state: MockState): boolean {
  return state.reauthAt !== null && nowSeconds() - state.reauthAt <= FRESH_WINDOW_SECONDS;
}

function checkInvite(state: MockState, invite: string): void {
  if (invitesClosed(state) || !state.registrationCodes.has(invite)) throw new MockHttpError(400, "invalid_invite", INVALID_INVITE);
}

function addPasskey(state: MockState, name: string | undefined): ReturnType<typeof newPasskey> {
  const passkey = newPasskey(state, name);
  state.passkeys.push(passkey);
  return passkey;
}

function newPasskey(state: MockState, name: string | undefined) {
  return {
    id: randomId("pk"),
    name: name?.trim() || `Passkey ${state.passkeys.length + 1}`,
    createdAt: nowSeconds(),
    lastUsedAt: null,
    deviceType: "multiDevice",
    backedUp: true,
    transports: ["internal"],
  };
}

function findPasskey(state: MockState, id: string) {
  const passkey = state.passkeys.find((item) => item.id === id);
  if (!passkey) throw new MockHttpError(404, "not_found", "Passkey not found");
  return passkey;
}

/**
 * The request behind a typed user code, only while pending and unexpired (the Worker's `pendingRequest`);
 * a malformed code, an unknown one, an expired one and a used one all read the same: 404 `not_found`.
 */
function pendingPairing(state: MockState, raw: string | undefined): MockPairing | null {
  const code = normalizeUserCode(raw ?? "");
  if (code.length !== USER_CODE_LENGTH || [...code].some((char) => !USER_CODE_ALPHABET.includes(char))) return null;
  const pairing = state.pairings.get(code);
  if (!pairing || pairing.status !== "pending") return null;
  if (pairing.expiresAt === null) return { ...pairing, expiresAt: nowSeconds() + PAIRING_TTL_SECONDS };
  return pairing.expiresAt > nowSeconds() ? pairing : null;
}

const pairingNotFound = () => new MockHttpError(404, "not_found", "Pairing code not found or expired");

// --- Handlers ---

const handlers: Record<string, Handler> = {
  "POST /api/auth/register/options": ({ state, body }) => {
    const invite = expectString(body.invite);
    const handle = expectString(body.handle);
    const displayName = expectString(body.displayName);
    checkInvite(state, invite);
    if (handle === state.user.handle) throw new MockHttpError(409, "conflict", "That handle is taken");
    const { challengeId, challenge } = issueChallenge(state, { purpose: "register", invite, handle, displayName });
    return { challengeId, options: creationOptions(challenge, { handle, displayName }) };
  },
  "POST /api/auth/register/verify": ({ state, body }) => {
    const challenge = takeChallenge(state, body.challengeId, "register");
    const invite = challenge.invite ?? "";
    checkInvite(state, invite);
    // The invite is consumed atomically with the account creation.
    state.registrationCodes.delete(invite);
    const now = nowSeconds();
    for (const created of state.invites) {
      if (state.inviteCodes.get(created.id) === invite) {
        created.usedAt = now;
        created.usedBy = challenge.handle ?? null;
      }
    }
    state.user = { id: randomId("user"), handle: challenge.handle ?? "new-user", displayName: challenge.displayName ?? "New User", role: "user" };
    state.passkeys = [];
    state.recoveryBatch = 0;
    addPasskey(state, undefined);
    state.invites = [];
    state.inviteCodes = new Map();
    startSession(state, "passkey");
    const recoveryCodes = newRecoveryBatch(state);
    // Like the Worker: `recovery_ack_at` stays NULL until `POST /api/recovery-codes/ack`.
    state.recoveryAckAt = null;
    return { user: state.user, recoveryCodes, batch: state.recoveryBatch };
  },

  "POST /api/auth/login/options": ({ state }) => {
    const { challengeId, challenge } = issueChallenge(state, { purpose: "login" });
    return { challengeId, options: requestOptions(challenge) };
  },
  "POST /api/auth/login/verify": ({ state, body }) => {
    takeChallenge(state, body.challengeId, "login");
    startSession(state, "passkey");
    const first = state.passkeys[0];
    if (first) first.lastUsedAt = nowSeconds();
    return { user: state.user };
  },

  "POST /api/auth/reauth/options": ({ state }) => {
    const { challengeId, challenge } = issueChallenge(state, { purpose: "reauth" });
    return { challengeId, options: requestOptions(challenge) };
  },
  "POST /api/auth/reauth/verify": ({ state, body }) => {
    takeChallenge(state, body.challengeId, "reauth");
    state.reauthAt = nowSeconds();
    return { ok: true, reauthAt: state.reauthAt };
  },

  "POST /api/auth/recover": ({ state, body }) => {
    if (state.recoverFailures >= MAX_RECOVER_FAILURES) {
      throw new MockHttpError(429, "rate_limited", "Too many attempts", { "Retry-After": "60" });
    }
    const code = normalizeRecoveryCode(expectString(body.code));
    if (expectString(body.handle) !== state.user.handle || !state.recoveryCodes.has(code)) {
      state.recoverFailures += 1;
      throw new MockHttpError(401, "unauthorized", "That handle or recovery code is not valid");
    }
    state.recoverFailures = 0;
    state.recoveryCodes.delete(code);
    startSession(state, "recovery");
    return { user: state.user, recoveryOnly: true, codesLeft: state.recoveryCodes.size };
  },
  "POST /api/auth/logout": ({ state }) => {
    state.signedIn = false;
    state.recoveryOnly = false;
    state.reauthAt = null;
    return { ok: true };
  },

  "GET /api/me": ({ state }) => meBody(state),
  "PATCH /api/me": ({ state, body }) => {
    state.user = { ...state.user, displayName: expectString(body.displayName).trim() };
    return { user: state.user };
  },

  "GET /api/sessions": ({ state }) => ({ sessions: state.sessions }),
  "DELETE /api/sessions/:id": ({ state, params }) => {
    const session = state.sessions.find((item) => item.id === params.id);
    if (!session) throw new MockHttpError(404, "not_found", "Session not found");
    state.sessions = state.sessions.filter((item) => item !== session);
    if (session.current) state.signedIn = false;
    return { ok: true };
  },

  "GET /api/passkeys": ({ state }) => ({ passkeys: state.passkeys }),
  "POST /api/passkeys/options": ({ state }) => {
    const { challengeId, challenge } = issueChallenge(state, { purpose: "add-passkey" });
    return { challengeId, options: creationOptions(challenge, state.user) };
  },
  "POST /api/passkeys/verify": ({ state, body }) => {
    takeChallenge(state, body.challengeId, "add-passkey");
    const passkey = addPasskey(state, typeof body.name === "string" ? body.name : undefined);
    // The Worker keeps a recovery session recovery-only after it added its passkey: the client
    // signs out and signs in with the new passkey.
    return { passkey };
  },
  "PATCH /api/passkeys/:id": ({ state, params, body }) => {
    const passkey = findPasskey(state, params.id ?? "");
    passkey.name = expectString(body.name).trim();
    return { passkey };
  },
  "DELETE /api/passkeys/:id": ({ state, params }) => {
    const passkey = findPasskey(state, params.id ?? "");
    if (state.passkeys.length <= 1) throw new MockHttpError(409, "last_passkey", "The last passkey cannot be removed");
    state.passkeys = state.passkeys.filter((item) => item !== passkey);
    return { ok: true };
  },

  "POST /api/recovery-codes/regenerate": ({ state }) => {
    const recoveryCodes = newRecoveryBatch(state);
    // Like the Worker: regenerating clears `recovery_ack_at` until `POST /api/recovery-codes/ack`.
    state.recoveryAckAt = null;
    return { recoveryCodes, batch: state.recoveryBatch };
  },
  "POST /api/recovery-codes/ack": ({ state, body }) => {
    // Like the Worker: only the current batch is acknowledged; a repeat for it is a no-op.
    if (body.batch !== state.recoveryBatch) throw new MockHttpError(409, "conflict", "These recovery codes are not the current ones");
    if (state.recoveryAckAt === null) state.recoveryAckAt = nowSeconds();
    return { ok: true };
  },

  "GET /api/invites": ({ state }) => ({ invites: state.invites, granted: state.invitesGranted, remaining: invitesRemaining(state) }),
  "POST /api/invites": ({ state }) => {
    // The real server answers 409: the user can do nothing about a paused gate, so it is a state conflict.
    if (invitesClosed(state)) throw new MockHttpError(409, "invites_paused", "Invites are paused");
    if (invitesRemaining(state) <= 0) throw new MockHttpError(409, "quota_exceeded", "No invites left");
    const now = nowSeconds();
    const invite = { id: randomId("inv"), createdAt: now, expiresAt: now + INVITE_TTL_SECONDS, usedAt: null, revokedAt: null, usedBy: null };
    const code = `DEV-INVITE-${base64url(randomBytes(4)).toUpperCase().replace(/[^A-Z0-9]/g, "X")}`;
    state.invites.push(invite);
    state.inviteCodes.set(invite.id, code);
    state.registrationCodes.add(code);
    return { invite, code };
  },
  "DELETE /api/invites/:id": ({ state, params }) => {
    const invite = state.invites.find((item) => item.id === params.id);
    if (!invite) throw new MockHttpError(404, "not_found", "Invite not found");
    if (invite.usedAt !== null || invite.revokedAt !== null) throw new MockHttpError(409, "conflict", "The invite is no longer active");
    invite.revokedAt = nowSeconds();
    state.registrationCodes.delete(state.inviteCodes.get(invite.id) ?? "");
    return { ok: true };
  },

  "GET /api/desktops": ({ state }) => ({ desktops: state.desktops }),
  "GET /api/pair/:userCode": ({ state, params }) => {
    const pairing = pendingPairing(state, params.userCode);
    if (!pairing) throw pairingNotFound();
    return {
      machineName: pairing.machineName,
      os: pairing.os,
      fingerprint: pairing.fingerprint,
      confirmationCode: pairing.confirmationCode,
      expiresAt: pairing.expiresAt,
    };
  },
  // Fresh gate and quota as the Worker: a new desktop is refused with 409 `quota_exceeded` at the limit
  // and the request stays pending; `replaces` needs one of the account's own desktops (404 otherwise).
  "POST /api/pair/:userCode/approve": ({ state, params, body }) => {
    const pairing = pendingPairing(state, params.userCode);
    const stored = state.pairings.get(normalizeUserCode(params.userCode ?? ""));
    if (!pairing || !stored) {
      if (stored?.status === "approved") throw new MockHttpError(409, "conflict", "Pairing request already approved");
      throw pairingNotFound();
    }
    const replaces = typeof body.replaces === "string" ? body.replaces : undefined;
    let result: { desktopId: string; name: string };
    if (replaces !== undefined) {
      const target = state.desktops.find((desktop) => desktop.id === replaces);
      if (!target) throw new MockHttpError(404, "not_found", "Desktop not found");
      target.os = pairing.os;
      target.pendingCount = 0;
      result = { desktopId: target.id, name: target.name };
    } else {
      if (state.desktops.length >= MAX_DESKTOPS) throw new MockHttpError(409, "quota_exceeded", "Desktop limit reached");
      const desktop = { id: randomId("desktop"), name: pairing.machineName, color: null, online: false, pendingCount: 0, os: pairing.os, appVersion: null, lastSeenAt: null };
      state.desktops.push(desktop);
      result = { desktopId: desktop.id, name: desktop.name };
    }
    stored.status = "approved";
    return result;
  },
  "POST /api/pair/:userCode/deny": ({ state, params }) => {
    const pairing = pendingPairing(state, params.userCode);
    const stored = state.pairings.get(normalizeUserCode(params.userCode ?? ""));
    if (!pairing || !stored) throw pairingNotFound();
    stored.status = "denied";
    return { ok: true };
  },

  // Push (P-01 contract): the key id changes only through `window.__monoptahMock` in a test.
  "GET /api/push/vapid-key": () => ({ publicKey: MOCK_VAPID_PUBLIC_KEY, keyId: MOCK_VAPID_KEY_ID }),
  "POST /api/push/subscriptions": ({ state, body }) => {
    const endpoint = expectString(body.endpoint);
    const deviceId = expectString(body.deviceId);
    if (body.vapidKeyId !== MOCK_VAPID_KEY_ID) throw new MockHttpError(400, "invalid", "Stale VAPID key: subscribe again with the current key");
    // The Worker upserts by endpoint: the same endpoint keeps one row.
    for (const [id, row] of state.pushSubscriptions) if (row.endpoint === endpoint) return { id };
    if ([...state.pushSubscriptions.values()].filter((row) => row.deviceId === deviceId).length >= 4) {
      throw new MockHttpError(409, "quota_exceeded", "Too many subscriptions on this device");
    }
    const id = randomId("push");
    state.pushSubscriptions.set(id, { endpoint, deviceId });
    return { id };
  },
  "DELETE /api/push/subscriptions/:id": ({ state, params }) => {
    if (!state.pushSubscriptions.delete(params.id ?? "")) throw new MockHttpError(404, "not_found", "Subscription not found");
    return { ok: true };
  },

  "GET /api/push/prefs/:desktopId": ({ state, params }) => {
    const id = params.desktopId ?? "";
    if (!state.desktops.some((desktop) => desktop.id === id)) throw new MockHttpError(404, "not_found", "Desktop not found");
    const stored = state.prefs.get(id);
    return { ...(stored ?? defaultPrefs()), stored: stored !== undefined };
  },
  "POST /api/push/test": ({ state, body }) => {
    const deviceId = expectString(body.deviceId);
    if (state.pushTest === "rate_limited") throw new MockHttpError(429, "rate_limited", "Too many tests");
    if (state.pushTest === "not_configured") throw new MockHttpError(503, "internal", "Push is not configured");
    const own = [...state.pushSubscriptions.values()].filter((row) => row.deviceId === deviceId);
    if (state.pushTest === "gone") return { sent: 0, results: own.map(() => ({ outcome: "gone", status: 410 })) };
    if (state.pushTest === "refused") return { sent: 0, results: own.map(() => ({ outcome: "failed", status: 403 })) };
    return { sent: own.length, results: own.map(() => ({ outcome: "ok", status: 201 })) };
  },
  "PUT /api/push/prefs/:desktopId": ({ state, params, body }) => {
    const id = params.desktopId ?? "";
    if (!state.desktops.some((desktop) => desktop.id === id)) throw new MockHttpError(404, "not_found", "Desktop not found");
    const prefs = { ...defaultPrefs(), ...(body as object) } as ReturnType<typeof defaultPrefs>;
    state.prefs.set(id, prefs);
    return prefs;
  },
};


// --- Admin (A-09): the semantics of web/worker/src/modules/admin ---

const ADMIN_PAGE_SIZE = 50;

/** Every account including the signed-in one, oldest first (Worker `LIST_USERS_SQL`). */
function allAdminUsers(state: MockState): MockAdminUser[] {
  const self: MockAdminUser = {
    id: state.user.id,
    handle: state.user.handle,
    displayName: state.user.displayName,
    role: state.user.role,
    status: "active",
    invitedBy: null,
    blockedVia: null,
    invitesGranted: state.invitesGranted,
    desktopCount: state.desktops.length,
    createdAt: nowSeconds() - 90 * 86400,
  };
  return [self, ...state.adminUsers].sort((a, b) => a.createdAt - b.createdAt || a.id.localeCompare(b.id));
}

function findAdminUser(state: MockState, id: string | undefined): MockAdminUser {
  const found = allAdminUsers(state).find((user) => user.id === id);
  if (!found) throw new MockHttpError(404, "not_found", "User not found");
  return found;
}

/** The user plus every descendant of the invite tree (the Worker's recursive CTE). */
function branchOf(state: MockState, rootId: string): MockAdminUser[] {
  const users = allAdminUsers(state);
  const out: MockAdminUser[] = [];
  const seen = new Set<string>();
  const queue = [rootId];
  while (queue.length > 0) {
    const id = queue.shift() as string;
    if (seen.has(id)) continue;
    seen.add(id);
    const user = users.find((item) => item.id === id);
    if (user) out.push(user);
    for (const child of users) if (child.invitedBy === id) queue.push(child.id);
  }
  return out;
}

function addAudit(state: MockState, event: Pick<MockAuditEvent, "action" | "targetType" | "targetId" | "meta">): void {
  state.audit.unshift({ id: randomId("aud"), ts: nowSeconds(), actorUserId: state.user.id, actorKind: "user", ...event });
}

/** Writes through to the signed-in account (kept in `state.user` and friends) or to a stored user. */
function stored(state: MockState, user: MockAdminUser): MockAdminUser | null {
  return state.adminUsers.find((item) => item.id === user.id) ?? null;
}

const adminHandlers: Record<string, Handler> = {
  "GET /api/admin/users": ({ state }) => ({ users: allAdminUsers(state) }),
  "POST /api/admin/users/:id/block": ({ state, params, body }) => {
    if (params.id === state.user.id) throw new MockHttpError(409, "conflict", "You cannot block yourself");
    const target = findAdminUser(state, params.id);
    const cascade = body.cascade === true;
    // The acting admin is never a member; a member already blocked is left as it was.
    const members = (cascade ? branchOf(state, target.id) : [target]).filter((user) => user.id !== state.user.id && user.status === "active");
    for (const member of members) {
      const row = stored(state, member);
      if (!row) continue;
      row.status = "blocked";
      row.blockedVia = cascade ? target.id : null;
    }
    if (members.length > 0) addAudit(state, { action: "user.block", targetType: "user", targetId: target.id, meta: { cascade, count: members.length } });
    return { blocked: members.length };
  },
  // Restores sign-in only: descendants stay blocked, revoked tokens and grants stay revoked.
  "POST /api/admin/users/:id/unblock": ({ state, params }) => {
    const target = findAdminUser(state, params.id);
    const row = stored(state, target);
    if (!row || row.status !== "blocked") throw new MockHttpError(409, "conflict", "User is not blocked");
    row.status = "active";
    row.blockedVia = null;
    addAudit(state, { action: "user.unblock", targetType: "user", targetId: target.id, meta: { cascade: false } });
    return { ok: true };
  },
  "POST /api/admin/users/:id/invites": ({ state, params, body }) => {
    const target = findAdminUser(state, params.id);
    const count = typeof body.count === "number" ? body.count : 0;
    const row = stored(state, target);
    const granted = row ? row.invitesGranted : state.invitesGranted;
    if (granted + count > MAX_INVITES_GRANTED) throw new MockHttpError(409, "conflict", `A user can hold at most ${MAX_INVITES_GRANTED} invites`);
    if (row) row.invitesGranted += count;
    else state.invitesGranted += count;
    addAudit(state, { action: "admin.invites_grant", targetType: "user", targetId: target.id, meta: { count } });
    return { ok: true };
  },
  "GET /api/admin/settings": ({ state }) => ({ invitesPaused: state.invitesPaused, maxUsers: state.maxUsers, userCount: userCount(state) }),
  "PUT /api/admin/settings": ({ state, body }) => {
    state.invitesPaused = body.invitesPaused === true;
    addAudit(state, { action: "admin.settings_change", targetType: "setting", targetId: "invites_paused", meta: { invitesPaused: state.invitesPaused } });
    return { invitesPaused: state.invitesPaused, maxUsers: state.maxUsers, userCount: userCount(state) };
  },
  // Newest first; the cursor is an offset here (the Worker's is an opaque `(ts, id)` pair).
  "GET /api/admin/audit": ({ state, query }) => {
    const limit = Number(query.get("limit") ?? ADMIN_PAGE_SIZE);
    const offset = Number(query.get("cursor") ?? 0);
    if (!Number.isInteger(limit) || limit < 1 || limit > 100 || !Number.isInteger(offset) || offset < 0) throw new MockHttpError(400, "invalid", "Invalid page query");
    const events = state.audit.slice(offset, offset + limit);
    return { events, nextCursor: offset + limit < state.audit.length ? String(offset + limit) : null };
  },
};

Object.assign(handlers, adminHandlers);

const compiled = Object.keys(handlers).map((key) => {
  const [method, path] = key.split(" ") as [string, string];
  const regex = new RegExp(`^${path.replace(/:([A-Za-z0-9_]+)/g, "(?<$1>[^/]+)")}$`);
  return { key, method, path, regex };
});

function json(status: number, body: unknown, headers: Record<string, string> = {}): Response {
  return new Response(JSON.stringify(body), { status, headers: { "Content-Type": "application/json", ...headers } });
}

function fail(error: MockHttpError): Response {
  return json(error.status, { error: error.message, code: error.code }, error.headers);
}

async function readBody(init: RequestInit): Promise<unknown> {
  if (typeof init.body !== "string" || init.body === "") return {};
  try {
    return JSON.parse(init.body);
  } catch {
    throw new MockHttpError(400, "invalid", "The body is not valid JSON");
  }
}

function header(init: RequestInit, name: string): string | null {
  return new Headers(init.headers).get(name);
}

export interface MockTransport {
  transport: ApiTransport;
  state: MockState;
}

export interface MockTransportOptions extends SeedOptions {
  /** Artificial delay per call, to see loading states in dev. */
  latencyMs?: number;
  /** Reuse an existing state (the dev hooks and the query-string presets share one). */
  state?: MockState;
}

export function createMockServer(options: MockTransportOptions = {}): MockTransport {
  const state = options.state ?? createMockState(options);
  const latencyMs = options.latencyMs ?? 0;

  const transport: ApiTransport = async (url, init) => {
    if (latencyMs > 0) await new Promise((resolve) => setTimeout(resolve, latencyMs));
    try {
      const method = (init.method ?? "GET").toUpperCase();
      const parsed = new URL(url, "http://mock.invalid");
      const match = compiled
        .filter((entry) => entry.method === method)
        .map((entry) => ({ entry, result: entry.regex.exec(parsed.pathname) }))
        .find((item) => item.result !== null);
      if (!match?.result) return json(404, { error: "Not found", code: "not_found" });
      const { entry } = match;
      const route = findRoute(entry.method, entry.path);
      if (!route) return json(404, { error: "Not found", code: "not_found" });

      if (method !== "GET") {
        if (header(init, "X-Monoptah-CSRF") !== "1") throw new MockHttpError(403, "csrf", "Missing CSRF header");
        if (!header(init, "Content-Type")?.startsWith("application/json")) {
          throw new MockHttpError(415, "unsupported_media_type", "Content-Type must be application/json");
        }
      }

      if (route.auth !== "public") {
        if (!state.signedIn) throw new MockHttpError(401, "unauthorized", "Sign in first");
        if (state.recoveryOnly) {
          // The Worker also lets a recovery-only session read GET /api/me and sign out.
          if (!route.allowRecoveryOnly && !RECOVERY_ONLY_ALLOWLIST.has(`${route.method} ${route.path}`)) throw new MockHttpError(403, "recovery_only", "This session can only add a passkey");
        } else if (route.auth === "admin" && state.user.role !== "admin") {
          throw new MockHttpError(403, "forbidden", "Not allowed");
        } else if ((route.auth === "fresh" || route.auth === "admin") && !isFresh(state)) {
          throw new MockHttpError(403, "fresh_required", "Confirm with your passkey first");
        }
      }

      const rawBody = await readBody(init);
      if (route.request.body) {
        const checked = route.request.body.safeParse(rawBody);
        if (!checked.success) throw new MockHttpError(400, "invalid", "The request body is not valid");
      }
      const params = { ...(match.result.groups ?? {}) } as Record<string, string>;
      for (const key of Object.keys(params)) params[key] = decodeURIComponent(params[key] ?? "");
      const result = handlers[entry.key]?.({ state, params, body: (rawBody ?? {}) as Record<string, unknown>, query: parsed.searchParams });
      state.counter += 1;
      return json(200, result);
    } catch (error) {
      if (error instanceof MockHttpError) return fail(error);
      throw error;
    }
  };

  return { transport, state };
}
