// Helpers for the R-03, R-04, R-14a and R-15a API tests: a room double, env overrides, a real pairing flow and
// token calls. Rows are written straight into D1 where a test does not need a route.
import { env } from "cloudflare:workers";
import { PAIRING_POLL_INTERVAL_SECONDS } from "@monoptah/protocol";
import { toBase64Url, toHex } from "../../../src/lib/crypto.ts";
import { BROWSER_HEADERS, call, callWithEnv } from "../../helpers.ts";
import { authedCall, createTestUser, freshIp, loginAs, type TestSession, type TestUser } from "../../support/accounts.ts";

export type TestEnv = typeof env;
export { PAIRING_POLL_INTERVAL_SECONDS };

export function randomKey(): { bytes: Uint8Array; b64: string; fingerprint: string } {
  const bytes = crypto.getRandomValues(new Uint8Array(32));
  return { bytes, b64: toBase64Url(bytes), fingerprint: "" };
}

/** Lowercase hex SHA-256 of raw bytes, computed the way the mock desktop does it. */
export async function sha256HexBytes(bytes: Uint8Array): Promise<string> {
  return toHex(new Uint8Array(await crypto.subtle.digest("SHA-256", bytes as BufferSource)));
}

export async function sha256Base64Url(bytes: Uint8Array): Promise<string> {
  return toBase64Url(new Uint8Array(await crypto.subtle.digest("SHA-256", bytes as BufferSource)));
}

export interface RoomCalls {
  revokes: { desktopId: string; target: unknown; code: number }[];
  notifies: string[];
}

/** A `ROOM` namespace double with revoke, notifyGrants and presence. `online` lists desktop ids that read as online. */
export function fakeRoom(online: Iterable<string> = []): { namespace: TestEnv["ROOM"]; calls: RoomCalls; online: Set<string> } {
  const calls: RoomCalls = { revokes: [], notifies: [] };
  const onlineIds = new Set(online);
  const namespace = {
    idFromName: (name: string) => ({ name }),
    get: (id: { name: string }) => ({
      revoke: (target: unknown, code: number) => {
        calls.revokes.push({ desktopId: id.name, target, code });
        return Promise.resolve();
      },
      notifyGrants: () => {
        calls.notifies.push(id.name);
        return Promise.resolve();
      },
      presence: () => Promise.resolve({ online: onlineIds.has(id.name) }),
    }),
  };
  return { namespace: namespace as unknown as TestEnv["ROOM"], calls, online: onlineIds };
}

/** The test environment with own values (bindings or vars) laid over it, without touching the real `env`. */
export function withEnv(values: Record<string, unknown>, base: TestEnv = env): TestEnv {
  const own = Object.fromEntries(Object.entries(values).map(([key, value]) => [key, { value, enumerable: true, writable: true, configurable: true }]));
  return Object.create(base, own) as TestEnv;
}

export interface Account {
  user: TestUser;
  session: TestSession;
}

/** A user with a fresh session (passkey re-auth just done). */
export async function newAccount(options: { fresh?: boolean; handle?: string } = {}): Promise<Account> {
  const user = await createTestUser(env, options.handle ? { handle: options.handle } : {});
  return { user, session: await loginAs(user.id, { fresh: options.fresh ?? true }) };
}

export function json(body: unknown, method = "POST"): RequestInit {
  return { method, body: JSON.stringify(body) };
}

/** A call with the desktop bearer token (no cookie, no browser headers needed). */
export function tokenCall(path: string, token: string | null, init: RequestInit = {}, testEnv: TestEnv = env): Promise<Response> {
  const headers: Record<string, string> = { "CF-Connecting-IP": freshIp(), "Content-Type": "application/json", ...(init.headers as Record<string, string> | undefined) };
  if (token !== null) headers.Authorization = `Bearer ${token}`;
  return callWithEnv(testEnv, path, { ...init, headers });
}

export interface Pairing {
  userCode: string;
  deviceCode: string;
  confirmationCode: string;
  verifier: string;
  publicKey: string;
  publicKeyBytes: Uint8Array;
  challenge: string;
  ip: string;
}

/** `pair/start` through the route, with a new key and verifier. Throws unless it answers 201. */
export async function startPairing(options: { ip?: string; machineName?: string; os?: string; testEnv?: TestEnv } = {}): Promise<Pairing> {
  const key = randomKey();
  const verifierBytes = crypto.getRandomValues(new Uint8Array(32));
  const challenge = await sha256Base64Url(verifierBytes);
  const ip = options.ip ?? freshIp();
  const res = await rawStart({ publicKey: key.b64, machineName: options.machineName ?? "Work Mac", os: options.os ?? "macos", appVersion: "1.0.0", challenge }, ip, options.testEnv);
  if (res.status !== 201) throw new Error(`pair/start answered ${res.status}: ${await res.text()}`);
  const body = (await res.json()) as { userCode: string; deviceCode: string; confirmationCode: string };
  return { ...body, verifier: toBase64Url(verifierBytes), publicKey: key.b64, publicKeyBytes: key.bytes, challenge, ip };
}

export function rawStart(body: unknown, ip: string = freshIp(), testEnv: TestEnv = env): Promise<Response> {
  return callWithEnv(testEnv, "/api/pair/start", { method: "POST", headers: { "CF-Connecting-IP": ip, "Content-Type": "application/json" }, body: JSON.stringify(body) });
}

/** `pair/poll`. The per-device 3 s window of the middleware is cleared first so the handler's own logic is what answers. */
export async function poll(deviceCode: string, verifier: string, testEnv: TestEnv = env): Promise<Response> {
  await env.DB.prepare("DELETE FROM rate_limits WHERE key LIKE 'pair-poll:device:%'").run();
  return callWithEnv(testEnv, "/api/pair/poll", {
    method: "POST",
    headers: { "CF-Connecting-IP": freshIp(), "Content-Type": "application/json" },
    body: JSON.stringify({ deviceCode, verifier }),
  });
}

export function approve(session: Pick<TestSession, "cookie">, userCode: string, body: unknown = {}, testEnv?: TestEnv): Promise<Response> {
  return authedCall(session, `/api/pair/${userCode}/approve`, json(body), testEnv);
}

export interface PairedDesktop {
  desktopId: string;
  token: string;
  pairing: Pairing;
}

/** A full pairing through the real routes: start, approve (fresh session), poll. Returns the desktop id and the token. */
export async function pairDesktop(account: Account, options: { machineName?: string; replaces?: string; testEnv?: TestEnv } = {}): Promise<PairedDesktop> {
  const pairing = await startPairing({ machineName: options.machineName, testEnv: options.testEnv });
  const res = await approve(account.session, pairing.userCode, options.replaces ? { replaces: options.replaces } : {}, options.testEnv);
  if (res.status !== 200) throw new Error(`approve answered ${res.status}: ${await res.text()}`);
  const polled = await poll(pairing.deviceCode, pairing.verifier, options.testEnv);
  const body = (await polled.json()) as { status: string; desktopId: string; desktopToken: string };
  if (body.status !== "approved") throw new Error(`poll answered ${body.status}`);
  return { desktopId: body.desktopId, token: body.desktopToken, pairing };
}

/** Registers a device through the route and returns its id and key. */
export async function registerDevice(account: Account, name = "Phone", testEnv?: TestEnv): Promise<{ deviceId: string; publicKey: string; bytes: Uint8Array }> {
  const key = randomKey();
  const res = await authedCall(account.session, "/api/devices", json({ name, publicKey: key.b64 }), testEnv);
  if (res.status !== 201 && res.status !== 200) throw new Error(`devices answered ${res.status}: ${await res.text()}`);
  return { deviceId: ((await res.json()) as { deviceId: string }).deviceId, publicKey: key.b64, bytes: key.bytes };
}

/** Requests a grant of `deviceId` on `desktopId` through the route. */
export function requestGrant(account: Account, desktopId: string, deviceId: string, testEnv?: TestEnv): Promise<Response> {
  return authedCall(account.session, `/api/desktops/${desktopId}/grants`, json({ deviceId }), testEnv);
}

/** The desktop reports a grant state with its token. */
export function reportState(token: string, deviceId: string, body: unknown, testEnv?: TestEnv): Promise<Response> {
  return tokenCall(`/api/desktop/grants/${deviceId}/state`, token, json(body), testEnv);
}

export async function grantRow(desktopId: string, deviceId: string): Promise<Record<string, unknown> | null> {
  return env.DB.prepare("SELECT * FROM device_grants WHERE desktop_id = ? AND device_id = ?").bind(desktopId, deviceId).first();
}

export async function auditActions(actorUserId: string): Promise<string[]> {
  const { results } = await env.DB.prepare("SELECT action FROM audit_log WHERE actor_user_id = ? ORDER BY ts, rowid").bind(actorUserId).all<{ action: string }>();
  return results.map((r) => r.action);
}

export { BROWSER_HEADERS, call, authedCall, env };
