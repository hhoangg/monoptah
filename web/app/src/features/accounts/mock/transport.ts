// Dev-only client-side stand-in for the Worker `/api/*` routes WS-S calls (A-03..A-08 contracts).
// It does not verify WebAuthn signatures: any well-formed response is accepted. The Worker rules it
// does imitate: Origin-free CSRF and content-type checks, session and fresh gates, `recovery_only`
// confinement, invite quota and pause, the last-passkey rule and the generic invalid-invite error.
import { findRoute, type ApiErrorCode } from "@monoptah/protocol";
import type { ApiTransport } from "../api";
import {
  base64url,
  createMockState,
  defaultPrefs,
  FRESH_WINDOW_SECONDS,
  newRecoveryBatch,
  normalizeRecoveryCode,
  nowSeconds,
  randomBytes,
  randomId,
  type MockChallenge,
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
}

type Handler = (ctx: Ctx) => unknown;

const CHALLENGE_TTL_SECONDS = 300;
const INVITE_TTL_SECONDS = 7 * 86400;
const MAX_RECOVER_FAILURES = 5;
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
  if (challenge.expiresAt < nowSeconds()) throw new MockHttpError(400, "expired", "The challenge expired");
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
  if (state.invitesPaused || !state.registrationCodes.has(invite)) throw new MockHttpError(400, "invalid_invite", INVALID_INVITE);
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
    addPasskey(state, undefined);
    state.invites = [];
    state.inviteCodes = new Map();
    startSession(state, "passkey");
    const recoveryCodes = newRecoveryBatch(state);
    // The codes are shown once on the screen the response feeds; the mock counts that as the gate.
    state.recoveryAckAt = now;
    return { user: state.user, recoveryCodes };
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
    // A recovery session that added its passkey becomes a normal session.
    if (state.recoveryOnly) {
      state.recoveryOnly = false;
      const current = state.sessions.find((item) => item.current);
      if (current) current.amr = "passkey";
    }
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
    state.recoveryAckAt = nowSeconds();
    return { recoveryCodes };
  },

  "GET /api/invites": ({ state }) => ({ invites: state.invites, granted: state.invitesGranted, remaining: invitesRemaining(state) }),
  "POST /api/invites": ({ state }) => {
    if (state.invitesPaused) throw new MockHttpError(403, "invites_paused", "Invites are paused");
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
  "PUT /api/push/prefs/:desktopId": ({ state, params, body }) => {
    const id = params.desktopId ?? "";
    if (!state.desktops.some((desktop) => desktop.id === id)) throw new MockHttpError(404, "not_found", "Desktop not found");
    const prefs = { ...defaultPrefs(), ...(body as object) } as ReturnType<typeof defaultPrefs>;
    state.prefs.set(id, prefs);
    return prefs;
  },
};

/** Routes a `recovery_only` session may call (A-04, A-05, A-08). */
const RECOVERY_ONLY_ALLOWED = new Set(["GET /api/me", "POST /api/auth/logout", "GET /api/passkeys", "POST /api/passkeys/options", "POST /api/passkeys/verify"]);

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
          if (!RECOVERY_ONLY_ALLOWED.has(entry.key)) throw new MockHttpError(403, "recovery_only", "This session can only add a passkey");
        } else if (route.auth === "fresh" && !isFresh(state)) {
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
      const result = handlers[entry.key]?.({ state, params, body: (rawBody ?? {}) as Record<string, unknown> });
      state.counter += 1;
      return json(200, result);
    } catch (error) {
      if (error instanceof MockHttpError) return fail(error);
      throw error;
    }
  };

  return { transport, state };
}
