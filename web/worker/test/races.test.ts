// Lost races and orphans: an audit row is written only by the call whose write took effect (invite revoke, admin
// unblock, session revoke, logout, block), and a sign-in or registration replaces the session cookie it carries.
import { env } from "cloudflare:workers";
import type { PublicKeyCredentialRequestOptionsJSON } from "@simplewebauthn/server";
import { describe, expect, it } from "vitest";
import { sha256Hex } from "../src/lib/crypto.ts";
import { publicSessionId } from "../src/modules/sessions/session.ts";
import { VirtualAuthenticator } from "./support/authenticator.ts";
import {
  authedCall,
  createTestDesktop,
  createTestInvite,
  createTestUser,
  fakeRoomNamespace,
  loginAs,
  postJson,
  registerOptions,
  registerUser,
  setCookies,
  withRoom,
} from "./support/accounts.ts";

const good = { origin: env.RP_ORIGIN, rpId: env.RP_ID };
const count = async (sql: string, ...bind: unknown[]) => (await env.DB.prepare(sql).bind(...bind).first<{ n: number }>())?.n ?? 0;
const auditCount = (action: string, actor: string) => count("SELECT count(*) AS n FROM audit_log WHERE action = ? AND actor_user_id = ?", action, actor);
const sessionExists = async (idHash: string) => (await count("SELECT count(*) AS n FROM sessions WHERE id_hash = ?", idHash)) === 1;

describe("audit rows of lost races", () => {
  it("invite revoke: two parallel revokes of one invite leave one invite.revoke row", async () => {
    const user = await createTestUser(env, { invitesGranted: 5 });
    const session = await loginAs(user.id, { fresh: true });
    const invite = await createTestInvite(env, { createdBy: user.id });
    const revoke = () => authedCall(session, `/api/invites/${invite.id}`, { method: "DELETE" });
    const statuses = (await Promise.all([revoke(), revoke()])).map((res) => res.status).sort();
    expect(statuses).toEqual([200, 409]);
    expect(await auditCount("invite.revoke", user.id)).toBe(1);
  });

  it("admin unblock: two parallel unblocks leave one user.unblock row", async () => {
    const admin = await createTestUser(env, { role: "admin" });
    const session = await loginAs(admin.id, { fresh: true });
    const target = await createTestUser(env, { status: "blocked" });
    const unblock = () => authedCall(session, `/api/admin/users/${target.id}/unblock`, { method: "POST" });
    const statuses = (await Promise.all([unblock(), unblock()])).map((res) => res.status).sort();
    expect(statuses).toEqual([200, 409]);
    expect(await auditCount("user.unblock", admin.id)).toBe(1);
  });

  it("session revoke: two parallel revokes of one session leave one session.revoke row and the loser is 404", async () => {
    const user = await createTestUser(env);
    const current = await loginAs(user.id);
    const other = await loginAs(user.id);
    const id = await publicSessionId(other.idHash);
    const revoke = () => authedCall(current, `/api/sessions/${id}`, { method: "DELETE" });
    const statuses = (await Promise.all([revoke(), revoke()])).map((res) => res.status).sort();
    expect(statuses).toEqual([200, 404]);
    expect(await auditCount("session.revoke", user.id)).toBe(1);
  });

  it("logout: two parallel logouts of one session leave one session.logout row", async () => {
    const user = await createTestUser(env);
    const session = await loginAs(user.id);
    const logout = () => authedCall(session, "/api/auth/logout", { method: "POST" });
    const statuses = (await Promise.all([logout(), logout()])).map((res) => res.status);
    for (const status of statuses) expect([200, 401]).toContain(status);
    expect(statuses).toContain(200);
    expect(await auditCount("session.logout", user.id)).toBe(1);
  });

  it("block: two parallel blocks of one user leave one user.block row, whose count is the rows the block changed", async () => {
    const admin = await createTestUser(env, { role: "admin" });
    const other = await createTestUser(env, { role: "admin" });
    const first = await loginAs(admin.id, { fresh: true });
    const second = await loginAs(other.id, { fresh: true });
    const target = await createTestUser(env);
    const body = JSON.stringify({ cascade: false, reason: "race" });
    const block = (session: typeof first) => authedCall(session, `/api/admin/users/${target.id}/block`, { method: "POST", body });
    const results = await Promise.all([block(first), block(second)]);
    expect(results.map((res) => res.status)).toEqual([200, 200]);
    const rows = (await env.DB.prepare("SELECT meta FROM audit_log WHERE action = 'user.block' AND target_id = ?").bind(target.id).all<{ meta: string }>()).results;
    expect(rows).toHaveLength(1);
    expect(JSON.parse(rows[0]?.meta ?? "{}")).toEqual({ cascade: false, count: 1 });
  });

  it("block: the count is computed from the rows changed, not read before the batch", async () => {
    const admin = await createTestUser(env, { role: "admin" });
    const session = await loginAs(admin.id, { fresh: true });
    const root = await createTestUser(env);
    const child = await createTestUser(env);
    await env.DB.prepare("UPDATE users SET invited_by = ? WHERE id = ?").bind(root.id, child.id).run();
    await env.DB.prepare("UPDATE users SET status = 'blocked', blocked_at = 1 WHERE id = ?").bind(child.id).run();
    const res = await authedCall(session, `/api/admin/users/${root.id}/block`, { method: "POST", body: JSON.stringify({ cascade: true, reason: "r" }) });
    expect(res.status).toBe(200);
    const row = await env.DB.prepare("SELECT meta FROM audit_log WHERE action = 'user.block' AND target_id = ?").bind(root.id).first<{ meta: string }>();
    expect(JSON.parse(row?.meta ?? "{}")).toEqual({ cascade: true, count: 1 });
  });
});

describe("a sign-in or registration replaces the session cookie it carries", () => {
  async function loginWithCookie(key: VirtualAuthenticator, cookie: string, testEnv?: typeof env) {
    const optionsRes = await postJson("/api/auth/login/options", {});
    const { challengeId, options } = (await optionsRes.json()) as { challengeId: string; options: PublicKeyCredentialRequestOptionsJSON };
    const response = await key.get(options, good);
    return postJson("/api/auth/login/verify", { challengeId, response }, { cookie, testEnv });
  }

  it("login deletes the old session row, closes its sockets, and leaves the new one", async () => {
    const user = await registerUser((await createTestInvite(env)).code);
    await createTestDesktop(env, user.userId);
    const oldHash = await sha256Hex(user.cookie.split("=")[1] ?? "");
    expect(await sessionExists(oldHash)).toBe(true);
    const room = fakeRoomNamespace();
    const res = await loginWithCookie(user.key, user.cookie, withRoom(env, room));
    expect(res.status).toBe(200);
    expect(await sessionExists(oldHash)).toBe(false);
    const newHash = await sha256Hex((setCookies(res)[0] ?? "").split(";")[0]?.split("=")[1] ?? "");
    expect(await sessionExists(newHash)).toBe(true);
    expect(await count("SELECT count(*) AS n FROM sessions WHERE user_id = ?", user.userId)).toBe(1);
    expect(room.calls.map((call) => call.target)).toEqual([{ sessionIdHashes: [oldHash] }]);
  });

  it("login with the cookie of another user's session removes that row too", async () => {
    const alice = await registerUser((await createTestInvite(env)).code);
    const bob = await registerUser((await createTestInvite(env)).code);
    const aliceHash = await sha256Hex(alice.cookie.split("=")[1] ?? "");
    expect((await loginWithCookie(bob.key, alice.cookie)).status).toBe(200);
    expect(await sessionExists(aliceHash)).toBe(false);
    expect(await count("SELECT count(*) AS n FROM sessions WHERE user_id = ?", bob.userId)).toBe(2);
  });

  it("a refused counter keeps the session of the cookie", async () => {
    const user = await registerUser((await createTestInvite(env)).code);
    const second = await loginAs(user.userId);
    // Bring the stored counter above what the key reports next.
    await env.DB.prepare("UPDATE passkeys SET counter = 100 WHERE id = ?").bind(user.key.credentialId).run();
    const res = await loginWithCookie(user.key, second.cookie);
    expect(res.status).toBe(401);
    expect(await sessionExists(second.idHash)).toBe(true);
  });

  it("registration deletes the session row of the cookie it carries", async () => {
    const existing = await loginAs((await createTestUser(env)).id);
    const key = await VirtualAuthenticator.generate();
    const attempt = await registerOptions((await createTestInvite(env)).code, `reg-${crypto.randomUUID().slice(0, 8)}`);
    const response = await key.create(attempt.options, good);
    const room = fakeRoomNamespace();
    await createTestDesktop(env, existing.userId);
    const res = await postJson("/api/auth/register/verify", { challengeId: attempt.challengeId, response }, { cookie: existing.cookie, testEnv: withRoom(env, room) });
    expect(res.status).toBe(200);
    expect(await sessionExists(existing.idHash)).toBe(false);
    expect(room.calls.map((call) => call.target)).toEqual([{ sessionIdHashes: [existing.idHash] }]);
    expect(await count("SELECT count(*) AS n FROM sessions WHERE id_hash = ?", await sha256Hex((setCookies(res)[0] ?? "").split(";")[0]?.split("=")[1] ?? ""))).toBe(1);
  });

  it("a failed registration keeps the session of the cookie", async () => {
    const existing = await loginAs((await createTestUser(env)).id);
    const key = await VirtualAuthenticator.generate();
    const invite = await createTestInvite(env);
    const attempt = await registerOptions(invite.code, `reg-${crypto.randomUUID().slice(0, 8)}`);
    const response = await key.create(attempt.options, good);
    await env.DB.prepare("UPDATE invites SET revoked_at = 1 WHERE id = ?").bind(invite.id).run();
    const res = await postJson("/api/auth/register/verify", { challengeId: attempt.challengeId, response }, { cookie: existing.cookie });
    expect(res.status).toBe(400);
    expect(await sessionExists(existing.idHash)).toBe(true);
  });
});
