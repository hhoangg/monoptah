// A-08: list, add, rename and delete passkeys with the virtual authenticator.
import { env } from "cloudflare:workers";
import { describe, expect, it } from "vitest";
import { VirtualAuthenticator } from "./support/authenticator.ts";
import { authedCall, createTestInvite, loginAs, loginWith, registerUser, type RegisteredUser } from "./support/accounts.ts";

const good = { origin: env.RP_ORIGIN, rpId: env.RP_ID };
const errorCode = async (res: Response) => ((await res.json()) as { code: string }).code;
const newUser = async (): Promise<RegisteredUser> => registerUser((await createTestInvite(env)).code);

interface PasskeyJson {
  id: string;
  name: string;
  createdAt: number;
  lastUsedAt: number | null;
  deviceType: string | null;
  backedUp: boolean;
  transports: string[];
}

async function listOf(cookie: string): Promise<PasskeyJson[]> {
  const res = await authedCall({ cookie }, "/api/passkeys");
  expect(res.status).toBe(200);
  return ((await res.json()) as { passkeys: PasskeyJson[] }).passkeys;
}

/** Runs the add ceremony with `key`. Returns the verify response. */
async function addPasskey(cookie: string, key: VirtualAuthenticator, name?: string): Promise<Response> {
  const optionsRes = await authedCall({ cookie }, "/api/passkeys/options", { method: "POST" });
  expect(optionsRes.status).toBe(200);
  const { challengeId, options } = (await optionsRes.json()) as { challengeId: string; options: Parameters<VirtualAuthenticator["create"]>[0] };
  const response = await key.create(options, good);
  return authedCall({ cookie }, "/api/passkeys/verify", { method: "POST", body: JSON.stringify({ challengeId, response, ...(name ? { name } : {}) }) });
}

const audit = async (userId: string, action: string) =>
  (await env.DB.prepare("SELECT meta, target_id FROM audit_log WHERE actor_user_id = ? AND action = ?").bind(userId, action).all<{ meta: string; target_id: string }>()).results;

/** A user with two passkeys. */
async function twoPasskeys() {
  const user = await newUser();
  const second = await VirtualAuthenticator.generate();
  expect((await addPasskey(user.cookie, second)).status).toBe(200);
  return { user, second };
}

describe("GET /api/passkeys", () => {
  it("lists the own passkeys only", async () => {
    const alice = await newUser();
    await newUser();
    const list = await listOf(alice.cookie);
    expect(list).toHaveLength(1);
    expect(list[0]).toMatchObject({ id: alice.key.credentialId, name: "Passkey", lastUsedAt: null, backedUp: false, transports: ["internal"] });
  });
});

describe("add a passkey", () => {
  it("works with a fresh session, names it and audits it", async () => {
    const user = await newUser();
    const second = await VirtualAuthenticator.generate();
    const res = await addPasskey(user.cookie, second);
    expect(res.status).toBe(200);
    const { passkey } = (await res.json()) as { passkey: PasskeyJson };
    expect(passkey).toMatchObject({ id: second.credentialId, name: "Passkey 2" });
    const named = await VirtualAuthenticator.generate();
    const { passkey: third } = (await (await addPasskey(user.cookie, named, "Work laptop")).json()) as { passkey: PasskeyJson };
    expect(third.name).toBe("Work laptop");
    expect(await listOf(user.cookie)).toHaveLength(3);
    const rows = await audit(user.userId, "passkey.add");
    expect(rows.map((r) => r.target_id).sort()).toEqual([second.credentialId, named.credentialId].sort());
    // The new passkey signs in.
    expect((await loginWith(second)).res.status).toBe(200);
  });

  it("options exclude the existing passkeys and are user verified", async () => {
    const user = await newUser();
    const res = await authedCall({ cookie: user.cookie }, "/api/passkeys/options", { method: "POST" });
    const { options } = (await res.json()) as { options: { excludeCredentials: { id: string }[]; authenticatorSelection: { userVerification: string; residentKey: string } } };
    expect(options.excludeCredentials.map((c) => c.id)).toEqual([user.key.credentialId]);
    expect(options.authenticatorSelection).toMatchObject({ userVerification: "required", residentKey: "required" });
  });

  it("needs a fresh session", async () => {
    const user = await newUser();
    await env.DB.prepare("UPDATE sessions SET reauth_at = ? WHERE user_id = ?").bind(1, user.userId).run();
    for (const path of ["/api/passkeys/options", "/api/passkeys/verify"]) {
      const res = await authedCall({ cookie: user.cookie }, path, { method: "POST", body: "{}" });
      expect(res.status, path).toBe(403);
      expect(await errorCode(res)).toBe("fresh_required");
    }
  });

  it("works with a recovery-only session, which stays recovery-only", async () => {
    const user = await newUser();
    const session = await loginAs(user.userId, { recoveryOnly: true });
    const second = await VirtualAuthenticator.generate();
    const res = await addPasskey(session.cookie, second);
    expect(res.status).toBe(200);
    const me = await authedCall(session, "/api/me");
    expect(((await me.json()) as { recoveryOnly: boolean }).recoveryOnly).toBe(true);
    expect((await loginWith(second)).res.status).toBe(200);
    expect(await audit(user.userId, "passkey.add")).toHaveLength(1);
  });

  it("refuses a credential that is already registered (409), here or for another user", async () => {
    const alice = await newUser();
    const bob = await newUser();
    const again = await addPasskey(alice.cookie, alice.key);
    expect(again.status).toBe(409);
    expect(await errorCode(again)).toBe("conflict");
    const stolen = await addPasskey(bob.cookie, alice.key);
    expect(stolen.status).toBe(409);
    expect(await listOf(alice.cookie)).toHaveLength(1);
    expect(await audit(alice.userId, "passkey.add")).toHaveLength(0);
  });

  it("a challenge is bound to its user and works once", async () => {
    const alice = await newUser();
    const bob = await newUser();
    const optionsRes = await authedCall({ cookie: alice.cookie }, "/api/passkeys/options", { method: "POST" });
    const { challengeId, options } = (await optionsRes.json()) as { challengeId: string; options: Parameters<VirtualAuthenticator["create"]>[0] };
    const key = await VirtualAuthenticator.generate();
    const response = await key.create(options, good);
    const foreign = await authedCall({ cookie: bob.cookie }, "/api/passkeys/verify", { method: "POST", body: JSON.stringify({ challengeId, response }) });
    expect(foreign.status).toBe(400);
    const ok = await authedCall({ cookie: alice.cookie }, "/api/passkeys/verify", { method: "POST", body: JSON.stringify({ challengeId, response }) });
    expect(ok.status).toBe(200);
    const replay = await authedCall({ cookie: alice.cookie }, "/api/passkeys/verify", { method: "POST", body: JSON.stringify({ challengeId, response }) });
    expect(replay.status).toBe(400);
  });

  it("refuses a wrong origin and a bad body", async () => {
    const user = await newUser();
    const optionsRes = await authedCall({ cookie: user.cookie }, "/api/passkeys/options", { method: "POST" });
    const { challengeId, options } = (await optionsRes.json()) as { challengeId: string; options: Parameters<VirtualAuthenticator["create"]>[0] };
    const response = await (await VirtualAuthenticator.generate()).create(options, { origin: "https://evil.example", rpId: env.RP_ID });
    expect((await authedCall({ cookie: user.cookie }, "/api/passkeys/verify", { method: "POST", body: JSON.stringify({ challengeId, response }) })).status).toBe(400);
    expect((await authedCall({ cookie: user.cookie }, "/api/passkeys/verify", { method: "POST", body: JSON.stringify({ nope: 1 }) })).status).toBe(400);
  });
});

describe("rename a passkey", () => {
  it("renames an own passkey (fresh) and audits it", async () => {
    const user = await newUser();
    const res = await authedCall({ cookie: user.cookie }, `/api/passkeys/${user.key.credentialId}`, { method: "PATCH", body: JSON.stringify({ name: "  Laptop  " }) });
    expect(res.status).toBe(200);
    expect(((await res.json()) as { passkey: PasskeyJson }).passkey.name).toBe("Laptop");
    expect((await listOf(user.cookie))[0]?.name).toBe("Laptop");
    expect((await audit(user.userId, "passkey.rename")).map((r) => r.target_id)).toEqual([user.key.credentialId]);
  });

  it("needs a fresh session and a valid name", async () => {
    const user = await newUser();
    const path = `/api/passkeys/${user.key.credentialId}`;
    expect((await authedCall({ cookie: user.cookie }, path, { method: "PATCH", body: JSON.stringify({ name: "" }) })).status).toBe(400);
    await env.DB.prepare("UPDATE sessions SET reauth_at = ? WHERE user_id = ?").bind(1, user.userId).run();
    const stale = await authedCall({ cookie: user.cookie }, path, { method: "PATCH", body: JSON.stringify({ name: "X" }) });
    expect(stale.status).toBe(403);
    expect(await errorCode(stale)).toBe("fresh_required");
  });

  it("answers 404 for another user's passkey and an unknown id, and changes nothing", async () => {
    const alice = await newUser();
    const bob = await newUser();
    for (const id of [alice.key.credentialId, "does-not-exist"]) {
      const res = await authedCall({ cookie: bob.cookie }, `/api/passkeys/${id}`, { method: "PATCH", body: JSON.stringify({ name: "Hacked" }) });
      expect(res.status).toBe(404);
    }
    expect((await listOf(alice.cookie))[0]?.name).toBe("Passkey");
    expect(await audit(bob.userId, "passkey.rename")).toHaveLength(0);
  });
});

describe("delete a passkey", () => {
  it("deletes one of two (fresh) and audits it with the count left", async () => {
    const { user, second } = await twoPasskeys();
    const res = await authedCall({ cookie: user.cookie }, `/api/passkeys/${second.credentialId}`, { method: "DELETE" });
    expect(res.status).toBe(200);
    expect(((await res.json()) as { ok: boolean }).ok).toBe(true);
    expect((await listOf(user.cookie)).map((p) => p.id)).toEqual([user.key.credentialId]);
    const rows = await audit(user.userId, "passkey.delete");
    expect(rows.map((r) => ({ id: r.target_id, meta: JSON.parse(r.meta) }))).toEqual([{ id: second.credentialId, meta: { passkeyId: second.credentialId, remaining: 1 } }]);
    // The deleted passkey no longer signs in.
    expect((await loginWith(second)).res.status).toBe(401);
  });

  it("never deletes the last passkey (409 last_passkey)", async () => {
    const user = await newUser();
    const res = await authedCall({ cookie: user.cookie }, `/api/passkeys/${user.key.credentialId}`, { method: "DELETE" });
    expect(res.status).toBe(409);
    expect(await errorCode(res)).toBe("last_passkey");
    expect(await listOf(user.cookie)).toHaveLength(1);
    expect(await audit(user.userId, "passkey.delete")).toHaveLength(0);
  });

  it("two parallel deletes of a user with 2 passkeys leave exactly 1", async () => {
    const { user, second } = await twoPasskeys();
    const [a, b] = await Promise.all([
      authedCall({ cookie: user.cookie }, `/api/passkeys/${user.key.credentialId}`, { method: "DELETE" }),
      authedCall({ cookie: user.cookie }, `/api/passkeys/${second.credentialId}`, { method: "DELETE" }),
    ]);
    expect([a.status, b.status].sort()).toEqual([200, 409]);
    const left = await env.DB.prepare("SELECT count(*) AS n FROM passkeys WHERE user_id = ?").bind(user.userId).first<{ n: number }>();
    expect(left?.n).toBe(1);
    expect(await audit(user.userId, "passkey.delete")).toHaveLength(1);
  });

  it("needs a fresh session", async () => {
    const { user, second } = await twoPasskeys();
    await env.DB.prepare("UPDATE sessions SET reauth_at = ? WHERE user_id = ?").bind(1, user.userId).run();
    const res = await authedCall({ cookie: user.cookie }, `/api/passkeys/${second.credentialId}`, { method: "DELETE" });
    expect(res.status).toBe(403);
    expect(await errorCode(res)).toBe("fresh_required");
    expect(await listOf(user.cookie)).toHaveLength(2);
  });

  it("answers 404 for another user's passkey and an unknown id, and deletes nothing", async () => {
    const alice = (await twoPasskeys()).user;
    const bob = await newUser();
    for (const id of [alice.key.credentialId, "does-not-exist"]) {
      expect((await authedCall({ cookie: bob.cookie }, `/api/passkeys/${id}`, { method: "DELETE" })).status).toBe(404);
    }
    expect(await listOf(alice.cookie)).toHaveLength(2);
    expect(await audit(bob.userId, "passkey.delete")).toHaveLength(0);
  });
});
