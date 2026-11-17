// Bounds on what the accounts routes accept: credential ids, transports, recovery codes and invite codes, and the
// two UNIQUE violations of registration.
import { env } from "cloudflare:workers";
import { describe, expect, it } from "vitest";
import { base64urlByteLength, boundedTransports, MAX_CREDENTIAL_ID_BYTES } from "../src/modules/passkeys/webauthn.ts";
import { MAX_CODE_LENGTH } from "../src/modules/recovery/recover.ts";
import { MAX_INVITE_LENGTH } from "../src/modules/auth/register.ts";
import { VirtualAuthenticator } from "./support/authenticator.ts";
import { authedCall, createTestInvite, createTestUser, loginWith, postJson, recordingEnv, registerOptions, registerUser } from "./support/accounts.ts";

const good = { origin: env.RP_ORIGIN, rpId: env.RP_ID };
const handleOf = (tag: string) => `${tag}-${crypto.randomUUID().slice(0, 8)}`;
const errorBody = async (res: Response) => (await res.json()) as { error: string; code: string };
const count = async (sql: string, ...bind: unknown[]) => (await env.DB.prepare(sql).bind(...bind).first<{ n: number }>())?.n ?? 0;

async function addPasskey(cookie: string, key: VirtualAuthenticator): Promise<Response> {
  const optionsRes = await authedCall({ cookie }, "/api/passkeys/options", { method: "POST" });
  const { challengeId, options } = (await optionsRes.json()) as { challengeId: string; options: Parameters<VirtualAuthenticator["create"]>[0] };
  const response = await key.create(options, good);
  return authedCall({ cookie }, "/api/passkeys/verify", { method: "POST", body: JSON.stringify({ challengeId, response }) });
}

describe("credential id length", () => {
  it("measures a base64url string without decoding it", () => {
    expect(base64urlByteLength("")).toBe(0);
    expect(base64urlByteLength("AAAA")).toBe(3);
    expect(base64urlByteLength("AAA")).toBe(2);
    expect(base64urlByteLength("AA")).toBe(1);
    expect(base64urlByteLength("A".repeat(1364))).toBe(1023);
    expect(base64urlByteLength("A".repeat(1368))).toBe(1026);
  });

  it.each([100, MAX_CREDENTIAL_ID_BYTES])("a %i byte credential id registers, signs in and adds as a second passkey", async (bytes) => {
    const key = await VirtualAuthenticator.generate(bytes);
    const user = await registerUser((await createTestInvite(env)).code, { key });
    expect(user.key.credentialId.length).toBeGreaterThan(128 * 1);
    const login = await loginWith(key);
    expect(login.res.status).toBe(200);
    expect(await count("SELECT count(*) AS n FROM audit_log WHERE action = 'account.login' AND actor_user_id = ?", user.userId)).toBe(1);

    const second = await VirtualAuthenticator.generate(bytes);
    const added = await addPasskey(login.cookie, second);
    expect(added.status).toBe(200);
    expect(await count("SELECT count(*) AS n FROM audit_log WHERE action = 'passkey.add' AND actor_user_id = ?", user.userId)).toBe(1);
    // A refused counter audits the long id too.
    expect((await loginWith(key, { counter: 0 })).res.status).toBe(401);
    expect(await count("SELECT count(*) AS n FROM audit_log WHERE action = 'account.login_refused_counter' AND actor_user_id = ?", user.userId)).toBe(1);
  });

  it("registration refuses a 1024 byte credential id with 400 invalid, before any write", async () => {
    const key = await VirtualAuthenticator.generate(MAX_CREDENTIAL_ID_BYTES + 1);
    const invite = await createTestInvite(env);
    const handle = handleOf("longid");
    const attempt = await registerOptions(invite.code, handle);
    const response = await key.create(attempt.options, good);
    const res = await postJson("/api/auth/register/verify", { challengeId: attempt.challengeId, response });
    expect(res.status).toBe(400);
    expect((await errorBody(res)).code).toBe("invalid");
    expect(await count("SELECT count(*) AS n FROM users WHERE handle = ?", handle)).toBe(0);
    expect(await count("SELECT count(*) AS n FROM passkeys WHERE id = ?", key.credentialId)).toBe(0);
    expect(await count("SELECT count(*) AS n FROM invites WHERE id = ? AND used_at IS NULL", invite.id)).toBe(1);
  });

  it("add-passkey refuses a 1024 byte credential id with 400 invalid and writes nothing", async () => {
    const user = await registerUser((await createTestInvite(env)).code);
    const long = await VirtualAuthenticator.generate(MAX_CREDENTIAL_ID_BYTES + 1);
    const recording = recordingEnv();
    const optionsRes = await authedCall({ cookie: user.cookie }, "/api/passkeys/options", { method: "POST" });
    const { challengeId, options } = (await optionsRes.json()) as { challengeId: string; options: Parameters<VirtualAuthenticator["create"]>[0] };
    const response = await long.create(options, good);
    const res = await authedCall({ cookie: user.cookie }, "/api/passkeys/verify", { method: "POST", body: JSON.stringify({ challengeId, response }) }, recording.testEnv);
    expect(res.status).toBe(400);
    expect((await errorBody(res)).code).toBe("invalid");
    expect(recording.batches()).toBe(0);
    expect(await count("SELECT count(*) AS n FROM passkeys WHERE user_id = ?", user.userId)).toBe(1);
  });
});

describe("stored transports", () => {
  it("keeps at most 8 entries of at most 32 characters", () => {
    expect(boundedTransports(["internal", "usb"])).toEqual(["internal", "usb"]);
    expect(boundedTransports(Array.from({ length: 20 }, (_, i) => `t${i}`))).toEqual(["t0", "t1", "t2", "t3", "t4", "t5", "t6", "t7"]);
    expect(boundedTransports(["x".repeat(33), "ok", ""])).toEqual(["ok"]);
    expect(boundedTransports(["x".repeat(32)])).toEqual(["x".repeat(32)]);
  });

  it("registration stores a bounded list however many an authenticator reports", async () => {
    const key = await VirtualAuthenticator.generate();
    const attempt = await registerOptions((await createTestInvite(env)).code, handleOf("trans"));
    const response = await key.create(attempt.options, good);
    response.response.transports = [...Array.from({ length: 30 }, (_, i) => `transport-${i}`), "y".repeat(500)] as never;
    const res = await postJson("/api/auth/register/verify", { challengeId: attempt.challengeId, response });
    expect(res.status).toBe(200);
    const row = await env.DB.prepare("SELECT transports FROM passkeys WHERE id = ?").bind(key.credentialId).first<{ transports: string }>();
    const stored = JSON.parse(row?.transports ?? "[]") as string[];
    expect(stored).toHaveLength(8);
    expect(stored.every((t) => t.length <= 32)).toBe(true);
  });
});

describe("unbounded inputs are refused before they are hashed", () => {
  it("a recovery code over 64 characters is the uniform 401 and spends neither a handle attempt nor a lookup", async () => {
    const user = await registerUser((await createTestInvite(env)).code);
    const recording = recordingEnv();
    const long = "A".repeat(MAX_CODE_LENGTH + 1);
    const res = await postJson("/api/auth/recover", { handle: user.handle, code: long }, { testEnv: recording.testEnv });
    expect(res.status).toBe(401);
    expect(await errorBody(res)).toEqual({ error: "Recovery failed", code: "unauthorized" });
    expect(recording.statements.filter((sql) => sql.includes("recovery_codes"))).toEqual([]);
    // Six of them do not lock the handle: its real code still works.
    for (let i = 0; i < 6; i += 1) expect((await postJson("/api/auth/recover", { handle: user.handle, code: long })).status).toBe(401);
    expect((await postJson("/api/auth/recover", { handle: user.handle, code: user.recoveryCodes[0] ?? "" })).status).toBe(200);
  });

  it("a recovery code of exactly 64 characters is still looked up (and fails the normal way)", async () => {
    const user = await registerUser((await createTestInvite(env)).code);
    const recording = recordingEnv();
    const res = await postJson("/api/auth/recover", { handle: user.handle, code: "A".repeat(MAX_CODE_LENGTH) }, { testEnv: recording.testEnv });
    expect(res.status).toBe(401);
    expect(recording.statements.filter((sql) => sql.includes("recovery_codes"))).toHaveLength(1);
  });

  it("an invite over 128 characters is invalid_invite and spends neither an invite attempt nor a lookup", async () => {
    const recording = recordingEnv();
    const attemptsBefore = await count("SELECT count(*) AS n FROM rate_limits WHERE key LIKE 'invite:code:%'");
    const res = await postJson("/api/auth/register/options", { invite: "i".repeat(MAX_INVITE_LENGTH + 1), handle: handleOf("long"), displayName: "X" }, { testEnv: recording.testEnv });
    expect(res.status).toBe(400);
    expect(await errorBody(res)).toEqual({ error: "This invitation is not valid", code: "invalid_invite" });
    expect(recording.statements.filter((sql) => sql.includes("invites"))).toEqual([]);
    expect(await count("SELECT count(*) AS n FROM rate_limits WHERE key LIKE 'invite:code:%'")).toBe(attemptsBefore);
    const exact = await postJson("/api/auth/register/options", { invite: "i".repeat(MAX_INVITE_LENGTH), handle: handleOf("long"), displayName: "X" }, { testEnv: recording.testEnv });
    expect(await errorBody(exact)).toEqual({ error: "This invitation is not valid", code: "invalid_invite" });
    expect(recording.statements.filter((sql) => sql.includes("FROM invites"))).toHaveLength(1);
  });
});

describe("register/verify UNIQUE violations", () => {
  it("a credential already registered on another account is 409 'Passkey already registered'", async () => {
    const first = await registerUser((await createTestInvite(env)).code);
    const invite = await createTestInvite(env);
    const handle = handleOf("again");
    const attempt = await registerOptions(invite.code, handle);
    const response = await first.key.create(attempt.options, good); // the same authenticator, another account
    const res = await postJson("/api/auth/register/verify", { challengeId: attempt.challengeId, response });
    expect(res.status).toBe(409);
    expect(await errorBody(res)).toEqual({ error: "Passkey already registered", code: "conflict" });
    expect(await count("SELECT count(*) AS n FROM users WHERE handle = ?", handle)).toBe(0);
    expect(await count("SELECT count(*) AS n FROM invites WHERE id = ? AND used_at IS NULL", invite.id)).toBe(1);
  });

  it("a handle taken between options and verify is 409 'Handle already taken'", async () => {
    const invite = await createTestInvite(env);
    const handle = handleOf("raced");
    const attempt = await registerOptions(invite.code, handle);
    const key = await VirtualAuthenticator.generate();
    const response = await key.create(attempt.options, good);
    await createTestUser(env, { handle });
    const res = await postJson("/api/auth/register/verify", { challengeId: attempt.challengeId, response });
    expect(res.status).toBe(409);
    expect(await errorBody(res)).toEqual({ error: "Handle already taken", code: "conflict" });
  });
});

