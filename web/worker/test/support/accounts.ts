// Test helpers for the accounts modules (WS-A and later agents). They write rows straight into D1, so a test
// does not need a passkey ceremony to be signed in.
//
//   const user = await createTestUser(env, { role: "admin" });
//   const session = await loginAs(user.id, { fresh: true });
//   const res = await authedCall(session, "/api/me");                       // GET
//   const res = await authedCall(session, "/api/me", { method: "PATCH", body: JSON.stringify({ displayName: "A" }) });
import { env } from "cloudflare:workers";
import { createSession, SESSION_COOKIE } from "../../src/modules/sessions/session.ts";
import { BROWSER_HEADERS, call, callWithEnv } from "../helpers.ts";

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

/** The test environment with `ROOM` replaced by a double. */
export function withRoom(testEnv: TestEnv, fake: { namespace: TestEnv["ROOM"] }): TestEnv {
  return Object.assign(Object.create(testEnv) as TestEnv, { ROOM: fake.namespace });
}

/** All `Set-Cookie` values of a response. */
export function setCookies(res: Response): string[] {
  return res.headers.getSetCookie();
}
