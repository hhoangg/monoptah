// Test helpers for the accounts modules (WS-A and later agents). They write rows straight into D1, so a test
// does not need a passkey ceremony to be signed in.
//
//   const user = await createTestUser(env, { role: "admin" });
//   const session = await loginAs(user.id, { fresh: true });
//   const res = await authedCall(session, "/api/me");                       // GET
//   const res = await authedCall(session, "/api/me", { method: "PATCH", body: JSON.stringify({ displayName: "A" }) });
import { env } from "cloudflare:workers";
import type { PublicKeyCredentialCreationOptionsJSON, PublicKeyCredentialRequestOptionsJSON } from "@simplewebauthn/server";
import { hashInviteCode, INVITE_TTL_SECONDS, newInviteCode } from "../../src/modules/invites/code.ts";
import { createSession, SESSION_COOKIE } from "../../src/modules/sessions/session.ts";
import { BROWSER_HEADERS, call, callWithEnv } from "../helpers.ts";
import { VirtualAuthenticator } from "./authenticator.ts";

type TestEnv = typeof env;

export interface TestUser {
  id: string;
  handle: string;
}

export interface CreateTestUserOptions {
  id?: string;
  /** 3-32 lowercase characters; random by default. */
  handle?: string;
  displayName?: string;
  role?: "user" | "admin";
  status?: "active" | "blocked";
  invitesGranted?: number;
  recoveryAckAt?: number | null;
}

/** Inserts a `users` row and returns its id and handle. */
export async function createTestUser(testEnv: TestEnv, options: CreateTestUserOptions = {}): Promise<TestUser> {
  const id = options.id ?? crypto.randomUUID();
  const handle = options.handle ?? `user-${crypto.randomUUID().slice(0, 8)}`;
  const status = options.status ?? "active";
  await testEnv.DB.prepare(
    "INSERT INTO users (id, handle, display_name, role, status, invites_granted, blocked_at, recovery_ack_at, created_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)",
  )
    .bind(
      id,
      handle,
      options.displayName ?? "Test User",
      options.role ?? "user",
      status,
      options.invitesGranted ?? 3,
      status === "blocked" ? 1 : null,
      options.recoveryAckAt ?? null,
      1,
    )
    .run();
  return { id, handle };
}

/** Inserts a `desktops` row for `userId` (room tests need desktops), and returns its id. */
export async function createTestDesktop(testEnv: TestEnv, userId: string, options: { id?: string; revoked?: boolean } = {}): Promise<string> {
  const id = options.id ?? crypto.randomUUID();
  await testEnv.DB.prepare(
    "INSERT INTO desktops (id, user_id, name, public_key, fingerprint, created_at, revoked_at) VALUES (?, ?, 'Desktop', 'key', 'fingerprint', 1, ?)",
  )
    .bind(id, userId, options.revoked ? 2 : null)
    .run();
  return id;
}

export interface TestSession {
  userId: string;
  token: string;
  /** `SHA-256(token)`: the row key. */
  idHash: string;
  /** Value of the `Cookie` request header. */
  cookie: string;
}

export interface LoginAsOptions {
  /** `reauth_at = now`: the session passes `fresh` and `admin` routes. */
  fresh?: boolean;
  /** A recovery-only session (30 minutes, `amr = 'recovery'`). */
  recoveryOnly?: boolean;
  userAgent?: string;
  /** Unix seconds of creation; defaults to now. */
  now?: number;
}

/** Inserts a session row for `userId` (as login would) and returns the cookie to send. */
export async function loginAs(userId: string, options: LoginAsOptions = {}, testEnv: TestEnv = env): Promise<TestSession> {
  const now = options.now ?? Math.floor(Date.now() / 1000);
  const created = await createSession(testEnv, {
    userId,
    amr: options.recoveryOnly ? "recovery" : "passkey",
    recoveryOnly: options.recoveryOnly ?? false,
    userAgent: options.userAgent ?? "vitest",
    reauthAt: options.fresh ? now : null,
    now,
  });
  await created.statement.run();
  return { userId, token: created.token, idHash: created.idHash, cookie: `${SESSION_COOKIE}=${created.token}` };
}

/**
 * A request with the session cookie. Non-GET calls also get the browser headers (Origin, CSRF, JSON), so they
 * pass `browserChecks`. `init.headers` wins over the defaults. Pass `testEnv` to swap a binding (see `fakeRoomNamespace`).
 */
export function authedCall(session: Pick<TestSession, "cookie">, path: string, init: RequestInit = {}, testEnv?: TestEnv): Promise<Response> {
  const method = init.method ?? "GET";
  const headers = { ...(method === "GET" ? {} : BROWSER_HEADERS), Cookie: session.cookie, ...(init.headers as Record<string, string> | undefined) };
  const request = { ...init, headers };
  return testEnv ? callWithEnv(testEnv, path, request) : call(path, request);
}

export interface RoomRevokeCall {
  /** The name the namespace was asked for: the desktop id. */
  desktopId: string;
  target: unknown;
  code: number;
}

/**
 * A `ROOM` namespace double that records every `room.revoke(target, code)` call. Use it as
 * `{ ...Object.create(env), ROOM: fake.namespace }`, i.e. `withRoom(env, fake)`.
 */
export function fakeRoomNamespace(): { namespace: TestEnv["ROOM"]; calls: RoomRevokeCall[] } {
  const calls: RoomRevokeCall[] = [];
  const namespace = {
    idFromName: (name: string) => ({ name }),
    get: (id: { name: string }) => ({
      revoke: (target: unknown, code: number) => {
        calls.push({ desktopId: id.name, target, code });
        return Promise.resolve();
      },
    }),
  };
  return { namespace: namespace as unknown as TestEnv["ROOM"], calls };
}

/**
 * An environment that reads `values` and falls back to `testEnv`. The values are own properties of a new object:
 * assigning through `Object.create(testEnv)` would call the setter of the real `env` and change it for every later
 * test of the file.
 */
function overlay(testEnv: TestEnv, values: Record<string, unknown>): TestEnv {
  const own = Object.fromEntries(Object.entries(values).map(([key, value]) => [key, { value, enumerable: true, writable: true, configurable: true }]));
  return Object.create(testEnv, own) as TestEnv;
}

/** The test environment with `ROOM` replaced by a double. */
export function withRoom(testEnv: TestEnv, fake: { namespace: TestEnv["ROOM"] }): TestEnv {
  return overlay(testEnv, { ROOM: fake.namespace });
}

/** All `Set-Cookie` values of a response. */
export function setCookies(res: Response): string[] {
  return res.headers.getSetCookie();
}

// --- Invites and passkey ceremonies (A-03, A-06) ---

export interface CreateTestInviteOptions {
  /** The inviter; defaults to a new user for a `user` invite. Ignored for `bootstrap` (created_by NULL). */
  createdBy?: string;
  kind?: "user" | "bootstrap";
  /** Unix seconds; defaults to now + 7 days. */
  expiresAt?: number;
  usedBy?: string;
  revoked?: boolean;
}

export interface TestInvite {
  /** The plain code, as a user would paste it. D1 holds only its SHA-256. */
  code: string;
  id: string;
  createdBy: string | null;
}

/** Inserts an invite row (hash only) and returns the plain code. Same hashing as scripts/bootstrap.mjs. */
export async function createTestInvite(testEnv: TestEnv, options: CreateTestInviteOptions = {}): Promise<TestInvite> {
  const kind = options.kind ?? "user";
  const createdBy = kind === "bootstrap" ? null : (options.createdBy ?? (await createTestUser(testEnv)).id);
  const code = newInviteCode();
  const id = crypto.randomUUID();
  const now = Math.floor(Date.now() / 1000);
  await testEnv.DB.prepare(
    "INSERT INTO invites (id, code_hash, created_by, kind, expires_at, used_by, used_at, revoked_at, created_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)",
  )
    .bind(
      id,
      await hashInviteCode(code),
      createdBy,
      kind,
      options.expiresAt ?? now + INVITE_TTL_SECONDS,
      options.usedBy ?? null,
      options.usedBy ? now : null,
      options.revoked ? now : null,
      now,
    )
    .run();
  return { code, id, createdBy };
}

/** The test environment with vars replaced, for example `withVars(env, { MAX_USERS: "2" })`. */
export function withVars(testEnv: TestEnv, vars: Partial<Record<"MAX_USERS" | "RP_ID" | "RP_ORIGIN", string>>): TestEnv {
  return overlay(testEnv, vars);
}

let ipCounter = 0;
/** A fresh fake client address, so a test never shares an IP rate limit window with another one. */
export function freshIp(): string {
  ipCounter += 1;
  return `10.${(ipCounter >> 16) & 255}.${(ipCounter >> 8) & 255}.${ipCounter & 255}`;
}

/** POST JSON through the Worker with the browser headers and a fresh client address. */
export function postJson(path: string, body: unknown, init: { cookie?: string; testEnv?: TestEnv; ip?: string } = {}): Promise<Response> {
  const headers: Record<string, string> = { "CF-Connecting-IP": init.ip ?? freshIp() };
  if (init.cookie) headers.Cookie = init.cookie;
  const request = { method: "POST", headers: { ...BROWSER_HEADERS, ...headers }, body: JSON.stringify(body) };
  return init.testEnv ? callWithEnv(init.testEnv, path, request) : call(path, request);
}

export interface RegistrationAttempt {
  options: PublicKeyCredentialCreationOptionsJSON;
  challengeId: string;
}

/** `register/options`; throws when it does not answer 200. */
export async function registerOptions(code: string, handle: string, init: { testEnv?: TestEnv; displayName?: string } = {}): Promise<RegistrationAttempt> {
  const res = await postJson("/api/auth/register/options", { invite: code, handle, displayName: init.displayName ?? "Test Person" }, init);
  if (res.status !== 200) throw new Error(`register/options answered ${res.status}: ${await res.text()}`);
  const body = (await res.json()) as { challengeId: string; options: PublicKeyCredentialCreationOptionsJSON };
  return body;
}

export interface RegisteredUser {
  userId: string;
  handle: string;
  key: VirtualAuthenticator;
  cookie: string;
  recoveryCodes: string[];
  role: string;
}

/** Registers a user through the real routes with a virtual authenticator. Throws when it does not succeed. */
export async function registerUser(
  code: string,
  options: { handle?: string; testEnv?: TestEnv; key?: VirtualAuthenticator } = {},
): Promise<RegisteredUser> {
  const handle = options.handle ?? `reg-${crypto.randomUUID().slice(0, 8)}`;
  const key = options.key ?? (await VirtualAuthenticator.generate());
  const attempt = await registerOptions(code, handle, options);
  const response = await key.create(attempt.options, { origin: env.RP_ORIGIN, rpId: env.RP_ID });
  const res = await postJson("/api/auth/register/verify", { challengeId: attempt.challengeId, response }, options);
  if (res.status !== 200) throw new Error(`register/verify answered ${res.status}: ${await res.text()}`);
  const body = (await res.json()) as { user: { id: string; role: string }; recoveryCodes: string[] };
  const cookie = setCookies(res)[0]?.split(";")[0] ?? "";
  return { userId: body.user.id, handle, key, cookie, recoveryCodes: body.recoveryCodes, role: body.user.role };
}

/** Signs in with `key` through the real routes; returns the response and the cookie (empty when none). */
export async function loginWith(
  key: VirtualAuthenticator,
  ceremony: Partial<Parameters<VirtualAuthenticator["get"]>[1]> = {},
  testEnv?: TestEnv,
): Promise<{ res: Response; cookie: string }> {
  const optionsRes = await postJson("/api/auth/login/options", {}, { testEnv });
  const { challengeId, options } = (await optionsRes.json()) as { challengeId: string; options: PublicKeyCredentialRequestOptionsJSON };
  const response = await key.get(options, { origin: env.RP_ORIGIN, rpId: env.RP_ID, ...ceremony });
  const res = await postJson("/api/auth/login/verify", { challengeId, response }, { testEnv });
  return { res, cookie: setCookies(res)[0]?.split(";")[0] ?? "" };
}
