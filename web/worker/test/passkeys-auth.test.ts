// A-03: passkey registration, login and re-auth through the real routes with the virtual authenticator.
import { env } from "cloudflare:workers";
import { describe, expect, it } from "vitest";
import { hashRecoveryCode } from "../src/modules/recovery/codes.ts";
import { CHALLENGE_TTL_SECONDS } from "../src/modules/passkeys/challenges.ts";
import { VirtualAuthenticator } from "./support/authenticator.ts";
import {
  authedCall,
  createTestInvite,
  loginAs,
  loginWith,
  postJson,
  registerOptions,
  registerUser,
  setCookies,
} from "./support/accounts.ts";

const good = { origin: env.RP_ORIGIN, rpId: env.RP_ID };

const errorCode = async (res: Response) => ((await res.json()) as { code: string }).code;

async function newUser() {
  const invite = await createTestInvite(env);
  return registerUser(invite.code);
}

async function reauthOptions(cookie: string) {
  const res = await authedCall({ cookie }, "/api/auth/reauth/options", { method: "POST" });
  expect(res.status).toBe(200);
  return (await res.json()) as { challengeId: string; options: Parameters<VirtualAuthenticator["get"]>[0] };
}

describe("register, login, re-auth", () => {
  it("runs the whole journey with a virtual authenticator", async () => {
    const invite = await createTestInvite(env);
    const user = await registerUser(invite.code, { handle: "journey-user" });
    expect(user.cookie).toMatch(/^__Host-mc_session=/);
    expect(user.role).toBe("user");

    // The registration session works.
    const me = await authedCall({ cookie: user.cookie }, "/api/me");
    expect(me.status).toBe(200);
    expect(((await me.json()) as { user: { handle: string } }).user.handle).toBe("journey-user");

    // Sign in again from scratch (discoverable: no username).
    const login = await loginWith(user.key);
    expect(login.res.status).toBe(200);
    expect(setCookies(login.res)[0]).toMatch(/^__Host-mc_session=.*HttpOnly.*Secure.*SameSite=Strict/);
    expect(((await login.res.json()) as { user: { id: string } }).user.id).toBe(user.userId);
    expect((await authedCall({ cookie: login.cookie }, "/api/me")).status).toBe(200);

    // The audit log has the register and the login.
    const { results } = await env.DB.prepare("SELECT action FROM audit_log WHERE actor_user_id = ? ORDER BY ts, action").bind(user.userId).all<{ action: string }>();
    expect(results.map((r) => r.action)).toEqual(expect.arrayContaining(["account.register", "invite.use", "account.login"]));
  });

  it("login options are discoverable with user verification required", async () => {
    const res = await postJson("/api/auth/login/options", {});
    expect(res.status).toBe(200);
    const body = (await res.json()) as { challengeId: string; options: { allowCredentials?: unknown; userVerification: string; rpId: string } };
    expect(body.options.allowCredentials).toBeUndefined();
    expect(body.options.userVerification).toBe("required");
    expect(body.options.rpId).toBe(env.RP_ID);
  });

  it("stores only what is needed of the passkey", async () => {
    const user = await newUser();
    const row = await env.DB.prepare("SELECT id, public_key, counter, transports, name FROM passkeys WHERE user_id = ?").bind(user.userId).first<{
      id: string;
      public_key: ArrayBuffer;
      counter: number;
      transports: string;
      name: string;
    }>();
    expect(row?.id).toBe(user.key.credentialId);
    expect(new Uint8Array(row?.public_key ?? new ArrayBuffer(0)).length).toBeGreaterThan(30);
    expect(row?.counter).toBe(0);
    expect(JSON.parse(row?.transports ?? "")).toEqual(["internal"]);
  });

  it("re-auth sets reauth_at and opens the fresh window", async () => {
    const user = await newUser();
    // Registration and login are passkey assertions: the session starts fresh. Age it first.
    const sessionHash = (await env.DB.prepare("SELECT id_hash FROM sessions WHERE user_id = ?").bind(user.userId).first<{ id_hash: string }>())?.id_hash ?? "";
    const stale = Math.floor(Date.now() / 1000) - 6 * 60;
    await env.DB.prepare("UPDATE sessions SET reauth_at = ? WHERE id_hash = ?").bind(stale, sessionHash).run();
    expect((await authedCall({ cookie: user.cookie }, "/api/passkeys/options", { method: "POST" })).status).toBe(403);

    const { challengeId, options } = await reauthOptions(user.cookie);
    expect(((options as unknown as { allowCredentials: { id: string }[] }).allowCredentials).map((c) => c.id)).toEqual([user.key.credentialId]);
    const response = await user.key.get(options, good);
    const res = await authedCall({ cookie: user.cookie }, "/api/auth/reauth/verify", { method: "POST", body: JSON.stringify({ challengeId, response }) });
    expect(res.status).toBe(200);
    const body = (await res.json()) as { ok: boolean; reauthAt: number };
    expect(body.ok).toBe(true);
    const row = await env.DB.prepare("SELECT reauth_at FROM sessions WHERE id_hash = ?").bind(sessionHash).first<{ reauth_at: number }>();
    expect(row?.reauth_at).toBe(body.reauthAt);
    // Fresh now: the fresh route passes the resolver and its handler answers the ceremony options.
    expect((await authedCall({ cookie: user.cookie }, "/api/passkeys/options", { method: "POST" })).status).toBe(200);

    // Six minutes later the window is closed again.
    await env.DB.prepare("UPDATE sessions SET reauth_at = ? WHERE id_hash = ?").bind(stale, sessionHash).run();
    const closed = await authedCall({ cookie: user.cookie }, "/api/passkeys/options", { method: "POST" });
    expect(closed.status).toBe(403);
    expect(await errorCode(closed)).toBe("fresh_required");
  });

  it("re-auth refuses a recovery-only session, another user's passkey and another user's challenge", async () => {
    const alice = await newUser();
    const bob = await newUser();

    const recoverySession = await loginAs(alice.userId, { recoveryOnly: true });
    const blocked = await authedCall(recoverySession, "/api/auth/reauth/options", { method: "POST" });
    expect(blocked.status).toBe(403);
    expect(await errorCode(blocked)).toBe("recovery_only");

    // Bob asserts with his own key on a challenge Alice asked for: the challenge is bound to Alice.
    const a = await reauthOptions(alice.cookie);
    const bobResponse = await bob.key.get({ ...a.options, allowCredentials: undefined }, good);
    const stolen = await authedCall({ cookie: bob.cookie }, "/api/auth/reauth/verify", { method: "POST", body: JSON.stringify({ challengeId: a.challengeId, response: bobResponse }) });
    expect(stolen.status).toBe(400);
    // The challenge was not burned by Bob: Alice can still use it.
    const aliceResponse = await alice.key.get(a.options, good);
    const ok = await authedCall({ cookie: alice.cookie }, "/api/auth/reauth/verify", { method: "POST", body: JSON.stringify({ challengeId: a.challengeId, response: aliceResponse }) });
    expect(ok.status).toBe(200);

    // Bob's own challenge, but Alice's passkey in the response: the credential must belong to the session user.
    const b = await reauthOptions(bob.cookie);
    const aliceOnBob = await alice.key.get({ ...b.options, allowCredentials: undefined }, good);
    const foreign = await authedCall({ cookie: bob.cookie }, "/api/auth/reauth/verify", { method: "POST", body: JSON.stringify({ challengeId: b.challengeId, response: aliceOnBob }) });
    expect(foreign.status).toBe(403);
  });
});

describe("challenges", () => {
  it("a challenge works once, even when the first attempt failed", async () => {
    const user = await newUser();
    // Login: failed first attempt (wrong origin) burns the challenge.
    const optionsRes = await postJson("/api/auth/login/options", {});
    const { challengeId, options } = (await optionsRes.json()) as { challengeId: string; options: Parameters<VirtualAuthenticator["get"]>[0] };
    const bad = await user.key.get(options, { ...good, origin: "https://evil.example" });
    expect((await postJson("/api/auth/login/verify", { challengeId, response: bad })).status).toBe(401);
    const retry = await user.key.get(options, good);
    const second = await postJson("/api/auth/login/verify", { challengeId, response: retry });
    expect(second.status).toBe(400);
    expect(setCookies(second)).toEqual([]);

    // Login: a replay of a successful assertion fails.
    const o2 = (await (await postJson("/api/auth/login/options", {})).json()) as { challengeId: string; options: Parameters<VirtualAuthenticator["get"]>[0] };
    const response = await user.key.get(o2.options, good);
    expect((await postJson("/api/auth/login/verify", { challengeId: o2.challengeId, response })).status).toBe(200);
    expect((await postJson("/api/auth/login/verify", { challengeId: o2.challengeId, response })).status).toBe(400);
  });

  it("a registration challenge works once", async () => {
    const invite = await createTestInvite(env);
    const key = await VirtualAuthenticator.generate();
    const attempt = await registerOptions(invite.code, `once-${crypto.randomUUID().slice(0, 8)}`);
    const response = await key.create(attempt.options, good);
    expect((await postJson("/api/auth/register/verify", { challengeId: attempt.challengeId, response })).status).toBe(200);
    expect((await postJson("/api/auth/register/verify", { challengeId: attempt.challengeId, response })).status).toBe(400);
  });

  it("expires after 5 minutes", async () => {
    const user = await newUser();
    expect(CHALLENGE_TTL_SECONDS).toBe(300);
    const { challengeId, options } = (await (await postJson("/api/auth/login/options", {})).json()) as { challengeId: string; options: Parameters<VirtualAuthenticator["get"]>[0] };
    const row = await env.DB.prepare("SELECT expires_at FROM challenges WHERE id = ?").bind(challengeId).first<{ expires_at: number }>();
    const now = Math.floor(Date.now() / 1000);
    expect((row?.expires_at ?? 0) - now).toBeGreaterThanOrEqual(298);
    expect((row?.expires_at ?? 0) - now).toBeLessThanOrEqual(300);
    await env.DB.prepare("UPDATE challenges SET expires_at = ? WHERE id = ?").bind(now - 1, challengeId).run();
    const response = await user.key.get(options, good);
    expect((await postJson("/api/auth/login/verify", { challengeId, response })).status).toBe(400);
  });

  it("is bound to its purpose", async () => {
    const user = await newUser();
    const { challengeId, options } = (await (await postJson("/api/auth/login/options", {})).json()) as { challengeId: string; options: Parameters<VirtualAuthenticator["get"]>[0] };
    const response = await user.key.get(options, good);
    // A login challenge presented to re-auth (as the session user) and to register is refused.
    expect((await authedCall({ cookie: user.cookie }, "/api/auth/reauth/verify", { method: "POST", body: JSON.stringify({ challengeId, response }) })).status).toBe(400);
    expect((await postJson("/api/auth/register/verify", { challengeId, response })).status).toBe(400);
    // It was not consumed by those mismatches.
    expect((await postJson("/api/auth/login/verify", { challengeId, response })).status).toBe(200);
  });
});

describe("ceremony checks", () => {
  it("rejects a wrong origin, a wrong RP ID and a missing UV flag at register", async () => {
    for (const bad of [{ ...good, origin: "https://evil.example" }, { ...good, rpId: "evil.example" }, { ...good, userVerified: false }]) {
      const invite = await createTestInvite(env);
      const key = await VirtualAuthenticator.generate();
      const attempt = await registerOptions(invite.code, `neg-${crypto.randomUUID().slice(0, 8)}`);
      const res = await postJson("/api/auth/register/verify", { challengeId: attempt.challengeId, response: await key.create(attempt.options, bad) });
      expect(res.status).toBe(400);
      const row = await env.DB.prepare("SELECT used_at FROM invites WHERE id = ?").bind(invite.id).first<{ used_at: number | null }>();
      expect(row?.used_at).toBeNull();
    }
  });

  it("rejects a wrong origin, a wrong RP ID and a missing UV flag at login", async () => {
    const user = await newUser();
    for (const bad of [{ origin: "https://evil.example" }, { rpId: "evil.example" }, { userVerified: false }]) {
      const { res, cookie } = await loginWith(user.key, bad);
      expect(res.status).toBe(401);
      expect(cookie).toBe("");
    }
  });

  it("refuses an unknown credential and a blocked user", async () => {
    const stranger = await VirtualAuthenticator.generate();
    stranger.userHandle = "c3RyYW5nZXI";
    expect((await loginWith(stranger)).res.status).toBe(401);

    const user = await newUser();
    await env.DB.prepare("UPDATE users SET status = 'blocked', blocked_at = 1 WHERE id = ?").bind(user.userId).run();
    const before = await env.DB.prepare("SELECT count(*) AS n FROM sessions WHERE user_id = ?").bind(user.userId).first<{ n: number }>();
    const { res } = await loginWith(user.key);
    expect(res.status).toBe(403);
    expect(await errorCode(res)).toBe("blocked");
    const after = await env.DB.prepare("SELECT count(*) AS n FROM sessions WHERE user_id = ?").bind(user.userId).first<{ n: number }>();
    expect(after?.n).toBe(before?.n);
  });
});

describe("signature counter", () => {
  const storedCounter = async (id: string) => (await env.DB.prepare("SELECT counter FROM passkeys WHERE id = ?").bind(id).first<{ counter: number }>())?.counter;

  it("refuses a counter that does not increase, keeps the stored one and audits it", async () => {
    const user = await newUser();
    expect((await loginWith(user.key, { counter: 5 })).res.status).toBe(200);
    expect(await storedCounter(user.key.credentialId)).toBe(5);
    for (const counter of [5, 3]) {
      const { res, cookie } = await loginWith(user.key, { counter });
      expect(res.status).toBe(401);
      expect(cookie).toBe("");
    }
    expect(await storedCounter(user.key.credentialId)).toBe(5);
    const { results } = await env.DB.prepare("SELECT meta FROM audit_log WHERE action = 'account.login_refused_counter' AND actor_user_id = ?").bind(user.userId).all<{ meta: string }>();
    expect(results).toHaveLength(2);
    expect(JSON.parse(results[0]?.meta ?? "{}").passkeyId).toBe(user.key.credentialId);
    expect((await loginWith(user.key, { counter: 6 })).res.status).toBe(200);
  });

  it("answers the same 401 without a counter-refusal audit when the passkey row vanished before the batch", async () => {
    const user = await newUser();
    const sessionsBefore = await sessionCount(user.userId);
    // A DB double whose batch first deletes the passkey: a passkey delete between the lookup and the batch.
    const realDb = env.DB;
    const vanishing = new Proxy(realDb, {
      get(target, prop) {
        if (prop === "batch") {
          return async (statements: D1PreparedStatement[]) => {
            await target.prepare("DELETE FROM passkeys WHERE id = ?").bind(user.key.credentialId).run();
            return target.batch(statements);
          };
        }
        const value = Reflect.get(target, prop, target) as unknown;
        return typeof value === "function" ? value.bind(target) : value;
      },
    });
    const testEnv = Object.create(env, { DB: { value: vanishing, enumerable: true } }) as typeof env;
    const { res, cookie } = await loginWith(user.key, { counter: 5 }, testEnv);
    expect(res.status).toBe(401);
    expect(cookie).toBe("");
    expect(await env.DB.prepare("SELECT count(*) AS n FROM passkeys WHERE id = ?").bind(user.key.credentialId).first<{ n: number }>()).toEqual({ n: 0 });
    expect(await refusals(user.userId)).toHaveLength(0);
    expect(await logins(user.userId)).toBe(0);
    expect(await sessionCount(user.userId)).toBe(sessionsBefore);
  });

  it("accepts an authenticator that keeps the counter at 0", async () => {
    const user = await newUser();
    expect((await loginWith(user.key, { counter: 0 })).res.status).toBe(200);
    expect((await loginWith(user.key, { counter: 0 })).res.status).toBe(200);
    expect(await storedCounter(user.key.credentialId)).toBe(0);
  });

  it("refuses a counter that drops to 0 on an authenticator that counted", async () => {
    const user = await newUser();
    expect((await loginWith(user.key, { counter: 2 })).res.status).toBe(200);
    expect((await loginWith(user.key, { counter: 0 })).res.status).toBe(401);
  });

  const refusals = async (userId: string) =>
    (await env.DB.prepare("SELECT meta FROM audit_log WHERE action = 'account.login_refused_counter' AND actor_user_id = ?").bind(userId).all<{ meta: string }>()).results;
  const logins = async (userId: string) => (await env.DB.prepare("SELECT count(*) AS n FROM audit_log WHERE action = 'account.login' AND actor_user_id = ?").bind(userId).first<{ n: number }>())?.n;
  const sessionCount = async (userId: string) => (await env.DB.prepare("SELECT count(*) AS n FROM sessions WHERE user_id = ?").bind(userId).first<{ n: number }>())?.n;
  const reauthWith = async (user: Awaited<ReturnType<typeof newUser>>, counter: number) => {
    const { challengeId, options } = await reauthOptions(user.cookie);
    const response = await user.key.get(options, { ...good, counter });
    return authedCall({ cookie: user.cookie }, "/api/auth/reauth/verify", { method: "POST", body: JSON.stringify({ challengeId, response }) });
  };

  it("re-auth applies the same rule, keeps reauth_at and audits the refusal", async () => {
    const user = await newUser();
    expect((await loginWith(user.key, { counter: 4 })).res.status).toBe(200);
    const stale = Math.floor(Date.now() / 1000) - 600;
    await env.DB.prepare("UPDATE sessions SET reauth_at = ? WHERE user_id = ?").bind(stale, user.userId).run();
    const before = (await refusals(user.userId)).length;
    const res = await reauthWith(user, 2);
    expect(res.status).toBe(403);
    expect(await storedCounter(user.key.credentialId)).toBe(4);
    const rows = await refusals(user.userId);
    expect(rows).toHaveLength(before + 1);
    expect(JSON.parse(rows[rows.length - 1]?.meta ?? "{}").passkeyId).toBe(user.key.credentialId);
    const fresh = await env.DB.prepare("SELECT count(*) AS n FROM sessions WHERE user_id = ? AND reauth_at = ?").bind(user.userId, stale).first<{ n: number }>();
    expect(fresh?.n).toBe(await sessionCount(user.userId));
  });

  it("two parallel logins with the same counter make exactly one session and one login event", async () => {
    const user = await newUser();
    const sessionsBefore = (await sessionCount(user.userId)) ?? 0;
    const loginsBefore = (await logins(user.userId)) ?? 0;
    const [a, b] = await Promise.all([loginWith(user.key, { counter: 7 }), loginWith(user.key, { counter: 7 })]);
    expect([a.res.status, b.res.status].sort()).toEqual([200, 401]);
    expect([a.cookie, b.cookie].filter((cookie) => cookie !== "")).toHaveLength(1);
    expect(await storedCounter(user.key.credentialId)).toBe(7);
    expect(await sessionCount(user.userId)).toBe(sessionsBefore + 1);
    expect(await logins(user.userId)).toBe(loginsBefore + 1);
    // The loser is audited like a counter decrease.
    expect(await refusals(user.userId)).toHaveLength(1);
  });

  it("two parallel re-auths with the same counter mark the session fresh once and audit the other", async () => {
    const user = await newUser();
    expect((await loginWith(user.key, { counter: 3 })).res.status).toBe(200);
    const stale = Math.floor(Date.now() / 1000) - 600;
    await env.DB.prepare("UPDATE sessions SET reauth_at = ? WHERE user_id = ?").bind(stale, user.userId).run();
    const [a, b] = await Promise.all([reauthWith(user, 9), reauthWith(user, 9)]);
    expect([a.status, b.status].sort()).toEqual([200, 403]);
    expect(await storedCounter(user.key.credentialId)).toBe(9);
    expect(await refusals(user.userId)).toHaveLength(1);
  });
});

describe("recovery codes at registration", () => {
  it("returns 10 Crockford codes once and stores only keyed hashes", async () => {
    const user = await newUser();
    expect(user.recoveryCodes).toHaveLength(10);
    expect(new Set(user.recoveryCodes).size).toBe(10);
    for (const code of user.recoveryCodes) expect(code).toMatch(/^[0-9A-HJKMNP-TV-Z]{4}(-[0-9A-HJKMNP-TV-Z]{4}){3}$/);

    const { results } = await env.DB.prepare("SELECT * FROM recovery_codes WHERE user_id = ?").bind(user.userId).all<Record<string, unknown>>();
    expect(results).toHaveLength(10);
    const dump = JSON.stringify(results);
    for (const code of user.recoveryCodes) {
      expect(dump).not.toContain(code);
      expect(dump).not.toContain(code.replaceAll("-", ""));
    }
    const expected = await Promise.all(user.recoveryCodes.map((code) => hashRecoveryCode(env, code)));
    expect(results.map((r) => r.code_hash).sort()).toEqual([...expected].sort());
    expect(results.every((r) => r.batch === 1 && r.used_at === null)).toBe(true);

    const row = await env.DB.prepare("SELECT recovery_ack_at FROM users WHERE id = ?").bind(user.userId).first<{ recovery_ack_at: number | null }>();
    expect(row?.recovery_ack_at).toBeNull();
  });

  it("normalizes Crockford aliases and case", async () => {
    expect(await hashRecoveryCode(env, "abcd-efgh-jkmn-pq01")).toBe(await hashRecoveryCode(env, "ABCD EFGH JKMN PQOI"));
  });

  it("writes no code or invite code to the audit log", async () => {
    const invite = await createTestInvite(env);
    const user = await registerUser(invite.code);
    const { results } = await env.DB.prepare("SELECT meta FROM audit_log WHERE actor_user_id = ?").bind(user.userId).all<{ meta: string }>();
    const dump = JSON.stringify(results);
    expect(dump).not.toContain(invite.code);
    for (const code of user.recoveryCodes) expect(dump).not.toContain(code);
  });
});
