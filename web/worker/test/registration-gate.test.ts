// A-06 registration part: the invite is consumed atomically, every invite or gate failure gives one generic
// error, and the pause switch and MAX_USERS cap are re-checked inside the batch that creates the user.
import { env } from "cloudflare:workers";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { resetInvalidCapLog } from "../src/modules/invites/gate.ts";
import { VirtualAuthenticator } from "./support/authenticator.ts";
import { authedCall, createTestInvite, createTestUser, loginAs, loginWith, postJson, registerOptions, registerUser, withVars } from "./support/accounts.ts";

const good = { origin: env.RP_ORIGIN, rpId: env.RP_ID };
const handleOf = (tag: string) => `${tag}-${crypto.randomUUID().slice(0, 8)}`;

beforeEach(async () => {
  await env.DB.prepare("DELETE FROM users").run();
  await env.DB.prepare("DELETE FROM settings").run();
});

const userCount = async () => (await env.DB.prepare("SELECT count(*) AS n FROM users").first<{ n: number }>())?.n ?? 0;
const setPaused = (value: "0" | "1") =>
  env.DB.prepare("INSERT INTO settings (key, value, updated_at) VALUES ('invites_paused', ?, 1) ON CONFLICT(key) DO UPDATE SET value = excluded.value").bind(value).run();

async function optionsBody(res: Response) {
  return { status: res.status, body: (await res.json()) as { code: string; error: string } };
}

/** Options for a registration, then the attestation, so the verify call can be made later or in parallel. */
async function prepare(code: string, handle: string, testEnv?: typeof env) {
  const key = await VirtualAuthenticator.generate();
  const attempt = await registerOptions(code, handle, { testEnv });
  const response = await key.create(attempt.options, good);
  return { key, handle, body: { challengeId: attempt.challengeId, response } };
}

const verify = (body: unknown, testEnv?: typeof env) => postJson("/api/auth/register/verify", body, { testEnv });

describe("invite validation", () => {
  it("answers every invite problem with the same generic error", async () => {
    const creator = await createTestUser(env);
    const used = await createTestInvite(env, { createdBy: creator.id, usedBy: creator.id });
    const revoked = await createTestInvite(env, { createdBy: creator.id, revoked: true });
    const expired = await createTestInvite(env, { createdBy: creator.id, expiresAt: Math.floor(Date.now() / 1000) - 1 });
    const answers = [];
    for (const invite of ["not-a-real-code", used.code, revoked.code, expired.code]) {
      const res = await postJson("/api/auth/register/options", { invite, handle: handleOf("x"), displayName: "X" });
      answers.push(await optionsBody(res));
    }
    for (const answer of answers) expect(answer).toEqual({ status: 400, body: { error: "This invitation is not valid", code: "invalid_invite" } });
  });

  it("cannot register without an invite", async () => {
    const res = await postJson("/api/auth/register/options", { handle: handleOf("x"), displayName: "X" });
    expect(res.status).toBe(400);
    expect(await userCount()).toBe(0);
  });

  it("answers handle taken only after the invite is valid", async () => {
    const taken = await createTestUser(env);
    const bad = await postJson("/api/auth/register/options", { invite: "nope", handle: taken.handle, displayName: "X" });
    expect(await optionsBody(bad)).toMatchObject({ status: 400, body: { code: "invalid_invite" } });
    const invite = await createTestInvite(env, { createdBy: taken.id });
    const conflict = await postJson("/api/auth/register/options", { invite: invite.code, handle: taken.handle, displayName: "X" });
    expect(await optionsBody(conflict)).toMatchObject({ status: 409, body: { code: "conflict" } });
  });

  it("re-checks the invite at verify: revoked, used or expired after the options", async () => {
    const creator = await createTestUser(env);
    for (const change of ["revoked_at = 1", "used_at = 1", "expires_at = 1"]) {
      const invite = await createTestInvite(env, { createdBy: creator.id });
      const attempt = await prepare(invite.code, handleOf("late"));
      await env.DB.prepare(`UPDATE invites SET ${change} WHERE id = ?`).bind(invite.id).run();
      const res = await verify(attempt.body);
      expect(await optionsBody(res)).toEqual({ status: 400, body: { error: "This invitation is not valid", code: "invalid_invite" } });
      expect(await env.DB.prepare("SELECT 1 FROM users WHERE handle = ?").bind(attempt.handle).first()).toBeNull();
      expect(await env.DB.prepare("SELECT 1 FROM passkeys WHERE id = ?").bind(attempt.key.credentialId).first()).toBeNull();
    }
  });

  it("the stored hash is the SHA-256 hex of the code (the bootstrap script contract)", async () => {
    const invite = await createTestInvite(env, { kind: "bootstrap" });
    const digest = await crypto.subtle.digest("SHA-256", new TextEncoder().encode(invite.code));
    const hex = [...new Uint8Array(digest)].map((b) => b.toString(16).padStart(2, "0")).join("");
    const row = await env.DB.prepare("SELECT code_hash FROM invites WHERE id = ?").bind(invite.id).first<{ code_hash: string }>();
    expect(row?.code_hash).toBe(hex);
    expect((await registerUser(invite.code)).role).toBe("admin");
  });
});

describe("invite consumption", () => {
  it("records the invite tree and marks the invite used", async () => {
    const inviter = await createTestUser(env);
    const invite = await createTestInvite(env, { createdBy: inviter.id });
    const user = await registerUser(invite.code);
    const row = await env.DB.prepare("SELECT invited_by, invite_id, role, status FROM users WHERE id = ?").bind(user.userId).first<Record<string, unknown>>();
    expect(row).toEqual({ invited_by: inviter.id, invite_id: invite.id, role: "user", status: "active" });
    const inv = await env.DB.prepare("SELECT used_by, used_at FROM invites WHERE id = ?").bind(invite.id).first<{ used_by: string; used_at: number }>();
    expect(inv?.used_by).toBe(user.userId);
    expect(inv?.used_at).toBeGreaterThan(0);
    // The code cannot be used again.
    const again = await postJson("/api/auth/register/options", { invite: invite.code, handle: handleOf("again"), displayName: "X" });
    expect((await optionsBody(again)).body.code).toBe("invalid_invite");
  });

  it("a bootstrap invite makes an admin with no inviter", async () => {
    const invite = await createTestInvite(env, { kind: "bootstrap" });
    const user = await registerUser(invite.code);
    expect(user.role).toBe("admin");
    const row = await env.DB.prepare("SELECT role, invited_by, invite_id FROM users WHERE id = ?").bind(user.userId).first<Record<string, unknown>>();
    expect(row).toEqual({ role: "admin", invited_by: null, invite_id: invite.id });
  });

  it("two parallel registrations with one code: exactly one succeeds and nothing of the other is written", async () => {
    const invite = await createTestInvite(env, { kind: "bootstrap" });
    const [a, b] = await Promise.all([prepare(invite.code, handleOf("pa")), prepare(invite.code, handleOf("pb"))]);
    const [ra, rb] = await Promise.all([verify(a.body), verify(b.body)]);
    expect([ra.status, rb.status].sort()).toEqual([200, 400]);
    const [winner, loser, loserAttempt] = ra.status === 200 ? [a, b, rb] : [b, a, ra];
    expect((await optionsBody(loserAttempt)).body.code).toBe("invalid_invite");
    expect(loserAttempt.headers.getSetCookie()).toEqual([]);
    expect(await userCount()).toBe(1);
    expect(await env.DB.prepare("SELECT 1 FROM users WHERE handle = ?").bind(loser.handle).first()).toBeNull();
    expect(await env.DB.prepare("SELECT 1 FROM users WHERE handle = ?").bind(winner.handle).first()).not.toBeNull();
    const counts = await env.DB.prepare("SELECT (SELECT count(*) FROM passkeys) AS passkeys, (SELECT count(*) FROM recovery_codes) AS codes, (SELECT count(*) FROM sessions) AS sessions, (SELECT count(*) FROM audit_log WHERE action = 'account.register' AND json_extract(meta, '$.inviteId') = ?1) AS registered, (SELECT count(*) FROM audit_log WHERE action = 'invite.use' AND json_extract(meta, '$.inviteId') = ?1) AS used").bind(invite.id).first();
    expect(counts).toEqual({ passkeys: 1, codes: 10, sessions: 1, registered: 1, used: 1 });
  });

  it("a handle taken between options and verify answers 409 and leaves the invite unused", async () => {
    const creator = await createTestUser(env);
    const handle = handleOf("race");
    const [i1, i2] = [await createTestInvite(env, { createdBy: creator.id }), await createTestInvite(env, { createdBy: creator.id })];
    const [a, b] = await Promise.all([prepare(i1.code, handle), prepare(i2.code, handle)]);
    const results = await Promise.all([verify(a.body), verify(b.body)]);
    expect(results.map((r) => r.status).sort()).toEqual([200, 409]);
    const used = await env.DB.prepare("SELECT count(*) AS n FROM invites WHERE used_at IS NOT NULL AND id IN (?, ?)").bind(i1.id, i2.id).first<{ n: number }>();
    expect(used?.n).toBe(1);
  });
});

describe("free-tier cap and pause switch", () => {
  async function seedUsers(count: number) {
    const users = [];
    for (let i = 0; i < count; i += 1) users.push(await createTestUser(env));
    return users;
  }

  it("MAX_USERS=2 with 2 users: the third registration fails with the generic error", async () => {
    const [u1] = await seedUsers(2);
    const capped = withVars(env, { MAX_USERS: "2" });
    const invite = await createTestInvite(env, { createdBy: u1?.id });
    const res = await postJson("/api/auth/register/options", { invite: invite.code, handle: handleOf("third"), displayName: "X" }, { testEnv: capped });
    expect(await optionsBody(res)).toEqual({ status: 400, body: { error: "This invitation is not valid", code: "invalid_invite" } });
    expect(await userCount()).toBe(2);
  });

  it("the cap is re-checked at verify", async () => {
    const [u1] = await seedUsers(1);
    const capped = withVars(env, { MAX_USERS: "2" });
    const invite = await createTestInvite(env, { createdBy: u1?.id });
    const attempt = await prepare(invite.code, handleOf("cap"), capped);
    await seedUsers(1);
    const res = await verify(attempt.body, capped);
    expect(await optionsBody(res)).toEqual({ status: 400, body: { error: "This invitation is not valid", code: "invalid_invite" } });
    expect(await userCount()).toBe(2);
    const row = await env.DB.prepare("SELECT used_at FROM invites WHERE id = ?").bind(invite.id).first<{ used_at: number | null }>();
    expect(row?.used_at).toBeNull();
  });

  it("two parallel registrations at MAX_USERS - 1 users: exactly one succeeds", async () => {
    const [u1] = await seedUsers(2);
    const capped = withVars(env, { MAX_USERS: "3" });
    const [i1, i2] = [await createTestInvite(env, { createdBy: u1?.id }), await createTestInvite(env, { createdBy: u1?.id })];
    const [a, b] = await Promise.all([prepare(i1.code, handleOf("ca"), capped), prepare(i2.code, handleOf("cb"), capped)]);
    const results = await Promise.all([verify(a.body, capped), verify(b.body, capped)]);
    expect(results.map((r) => r.status).sort()).toEqual([200, 400]);
    expect(await userCount()).toBe(3);
  });

  it.each(["", "abc", "-1", "1.5", " 3", "1e3", "0x10"])("an invalid MAX_USERS (%j) fails closed: options, verify and invite creation stop, one error line is logged", async (value) => {
    const [u1] = await seedUsers(1);
    const good = await createTestInvite(env, { createdBy: u1?.id });
    const attempt = await prepare(good.code, handleOf("pre")); // options while the gate is open
    const broken = withVars(env, { MAX_USERS: value });
    resetInvalidCapLog();
    const errors = vi.spyOn(console, "error").mockImplementation(() => undefined);
    try {
      const res = await postJson("/api/auth/register/options", { invite: good.code, handle: handleOf("closed"), displayName: "X" }, { testEnv: broken });
      expect(await optionsBody(res)).toEqual({ status: 400, body: { error: "This invitation is not valid", code: "invalid_invite" } });
      const verified = await verify(attempt.body, broken);
      expect(await optionsBody(verified)).toEqual({ status: 400, body: { error: "This invitation is not valid", code: "invalid_invite" } });
      expect(await userCount()).toBe(1);
      const session = await loginAs(u1?.id ?? "", { fresh: true });
      const created = await authedCall(session, "/api/invites", { method: "POST" }, broken);
      expect(created.status).toBe(409);
      expect(((await created.json()) as { code: string }).code).toBe("invites_paused");
      const logged = errors.mock.calls.map((args) => String(args[0]));
      expect(logged).toHaveLength(1);
      expect(logged[0]).toMatch(/^MAX_USERS is not a non-negative integer/);
      if (value.length > 2) expect(logged[0]).not.toContain(value);
    } finally {
      errors.mockRestore();
    }
  });

  it("the admin settings show a closed gate as a cap that is reached, never 0 (no cap)", async () => {
    await seedUsers(2);
    const admin = await createTestUser(env, { role: "admin" });
    const session = await loginAs(admin.id, { fresh: true });
    const res = await authedCall(session, "/api/admin/settings", {}, withVars(env, { MAX_USERS: "oops" }));
    expect(res.status).toBe(200);
    const body = (await res.json()) as { maxUsers: number; userCount: number };
    expect(body.maxUsers).toBe(body.userCount);
    expect(body.maxUsers).toBeGreaterThan(0);
  });

  it("MAX_USERS=0 means no cap", async () => {
    const [u1] = await seedUsers(5);
    const open = withVars(env, { MAX_USERS: "0" });
    const invite = await createTestInvite(env, { createdBy: u1?.id });
    const user = await registerUser(invite.code, { testEnv: open });
    expect(user.userId).toBeTruthy();
    expect(await userCount()).toBe(6);
  });

  it("invites_paused=1 fails options and verify the same way, and sign-in still works", async () => {
    const existing = await registerUser((await createTestInvite(env, { kind: "bootstrap" })).code);
    const [u1] = await seedUsers(1);
    const invite = await createTestInvite(env, { createdBy: u1?.id });
    const early = await prepare(invite.code, handleOf("early"));
    await setPaused("1");

    const options = await postJson("/api/auth/register/options", { invite: invite.code, handle: handleOf("p"), displayName: "X" });
    expect(await optionsBody(options)).toEqual({ status: 400, body: { error: "This invitation is not valid", code: "invalid_invite" } });
    const verified = await verify(early.body);
    expect(await optionsBody(verified)).toEqual({ status: 400, body: { error: "This invitation is not valid", code: "invalid_invite" } });
    expect(await env.DB.prepare("SELECT 1 FROM users WHERE handle = ?").bind(early.handle).first()).toBeNull();

    expect((await loginWith(existing.key)).res.status).toBe(200);

    await setPaused("0");
    expect((await registerUser(invite.code)).userId).toBeTruthy();
  });
});
