// A-06 invite routes: create (quota, gate, code shown once), list and revoke.
import { env } from "cloudflare:workers";
import { beforeEach, describe, expect, it } from "vitest";
import { ApiErrorSchema, CreateInviteResponse, InvitesResponse, MeResponse } from "@monoptah/protocol";
import { hashInviteCode } from "../src/modules/invites/code.ts";
import { authedCall, createTestInvite, createTestUser, loginAs, recordingEnv, registerOptions, withVars } from "./support/accounts.ts";

beforeEach(async () => {
  await env.DB.prepare("DELETE FROM users").run();
  await env.DB.prepare("DELETE FROM settings").run();
  await env.DB.prepare("DELETE FROM audit_log").run();
});

const create = (session: { cookie: string }, testEnv?: typeof env) => authedCall(session, "/api/invites", { method: "POST" }, testEnv);
const list = async (session: { cookie: string }) => InvitesResponse.parse(await (await authedCall(session, "/api/invites")).json());
const revoke = (session: { cookie: string }, id: string) => authedCall(session, `/api/invites/${id}`, { method: "DELETE" });
const errorCode = async (res: Response) => ApiErrorSchema.parse(await res.json()).code;
const setPaused = (value: "0" | "1") =>
  env.DB.prepare("INSERT INTO settings (key, value, updated_at) VALUES ('invites_paused', ?, 1) ON CONFLICT(key) DO UPDATE SET value = excluded.value").bind(value).run();
const audits = async (action: string) =>
  (await env.DB.prepare("SELECT actor_user_id, actor_kind, target_id, meta FROM audit_log WHERE action = ?").bind(action).all<{ actor_user_id: string; actor_kind: string; target_id: string; meta: string }>()).results;

describe("POST /api/invites", () => {
  it("creates an invite: the code is shown once, D1 keeps only its SHA-256, expiry is 7 days", async () => {
    const user = await createTestUser(env);
    const res = await create(await loginAs(user.id));
    expect(res.status).toBe(201);
    const body = CreateInviteResponse.parse(await res.json());
    expect(body.code).toMatch(/^[A-Za-z0-9_-]{22}$/);
    const row = await env.DB.prepare("SELECT code_hash, created_by, kind, expires_at, created_at FROM invites WHERE id = ?").bind(body.invite.id).first<{ code_hash: string; created_by: string; kind: string; expires_at: number; created_at: number }>();
    expect(row?.code_hash).toBe(await hashInviteCode(body.code));
    expect(row?.code_hash).toMatch(/^[0-9a-f]{64}$/);
    expect(row).toMatchObject({ created_by: user.id, kind: "user" });
    expect(row?.expires_at).toBe((row?.created_at ?? 0) + 7 * 86400);
    expect(body.invite).toMatchObject({ expiresAt: row?.expires_at, usedAt: null, revokedAt: null, usedBy: null });
    // The code is never stored in clear and never listed.
    expect(JSON.stringify(await list(await loginAs(user.id)))).not.toContain(body.code);
  });

  it("the new code registers (options accepts it)", async () => {
    const user = await createTestUser(env);
    const { code } = CreateInviteResponse.parse(await (await create(await loginAs(user.id))).json());
    expect((await registerOptions(code, `new-${crypto.randomUUID().slice(0, 8)}`)).options.challenge).toBeTruthy();
  });

  it("audits invite.create without the code", async () => {
    const user = await createTestUser(env);
    const body = CreateInviteResponse.parse(await (await create(await loginAs(user.id))).json());
    const [event, ...rest] = await audits("invite.create");
    expect(rest).toEqual([]);
    expect(event).toMatchObject({ actor_user_id: user.id, actor_kind: "user", target_id: body.invite.id });
    expect(JSON.parse(event?.meta ?? "")).toEqual({ inviteId: body.invite.id });
    expect(JSON.stringify(event)).not.toContain(body.code);
  });

  it("quota: 3 invites, the fourth is quota_exceeded and writes nothing", async () => {
    const user = await createTestUser(env);
    const session = await loginAs(user.id);
    for (let i = 0; i < 3; i += 1) expect((await create(session)).status).toBe(201);
    const res = await create(session);
    expect(res.status).toBe(409);
    expect(await errorCode(res)).toBe("quota_exceeded");
    expect((await list(session)).invites).toHaveLength(3);
    expect(await audits("invite.create")).toHaveLength(3);
  });

  it("quota math: a revoked unused invite frees a slot, a used one does not", async () => {
    const user = await createTestUser(env, { invitesGranted: 2 });
    const session = await loginAs(user.id);
    const first = CreateInviteResponse.parse(await (await create(session)).json());
    const second = CreateInviteResponse.parse(await (await create(session)).json());
    expect(await errorCode(await create(session))).toBe("quota_exceeded");
    expect((await revoke(session, first.invite.id)).status).toBe(200);
    expect((await create(session)).status).toBe(201);
    expect(await errorCode(await create(session))).toBe("quota_exceeded");
    // A used invite keeps its slot: mark `second` used.
    const invitee = await createTestUser(env);
    await env.DB.prepare("UPDATE invites SET used_by = ?, used_at = 5 WHERE id = ?").bind(invitee.id, second.invite.id).run();
    expect(await errorCode(await create(session))).toBe("quota_exceeded");
  });

  it("two parallel creates at one remaining slot: exactly one succeeds", async () => {
    const user = await createTestUser(env, { invitesGranted: 1 });
    const session = await loginAs(user.id);
    const results = await Promise.all([create(session), create(session), create(session)]);
    expect(results.map((r) => r.status).sort()).toEqual([201, 409, 409]);
    const count = await env.DB.prepare("SELECT count(*) AS n FROM invites WHERE created_by = ?").bind(user.id).first<{ n: number }>();
    expect(count?.n).toBe(1);
    expect(await audits("invite.create")).toHaveLength(1);
  });

  it("MAX_USERS=2 with 2 users answers invites_paused", async () => {
    const user = await createTestUser(env);
    await createTestUser(env);
    const capped = withVars(env, { MAX_USERS: "2" });
    const res = await create(await loginAs(user.id), capped);
    expect(res.status).toBe(409);
    expect(await errorCode(res)).toBe("invites_paused");
    expect(await audits("invite.create")).toHaveLength(0);
    // One user fewer than the cap: allowed again.
    const roomy = withVars(env, { MAX_USERS: "3" });
    expect((await create(await loginAs(user.id), roomy)).status).toBe(201);
  });

  it("MAX_USERS=0 means no cap", async () => {
    const user = await createTestUser(env);
    for (let i = 0; i < 5; i += 1) await createTestUser(env);
    expect((await create(await loginAs(user.id), withVars(env, { MAX_USERS: "0" }))).status).toBe(201);
  });

  it("the pause switch answers invites_paused even with quota left, and unpausing restores it", async () => {
    const user = await createTestUser(env);
    const session = await loginAs(user.id);
    await setPaused("1");
    const res = await create(session);
    expect(res.status).toBe(409);
    expect(await errorCode(res)).toBe("invites_paused");
    await setPaused("0");
    expect((await create(session)).status).toBe(201);
  });

  it("requires a session", async () => {
    const res = await authedCall({ cookie: "" }, "/api/invites", { method: "POST" });
    expect(res.status).toBe(401);
  });
});

describe("POST /api/invites is one batch", () => {
  it("when the batch fails no invite and no audit row is written, and the slot is kept", async () => {
    const user = await createTestUser(env);
    const session = await loginAs(user.id);
    const failing = recordingEnv({ failBatch: true });
    const res = await create(session, failing.testEnv);
    expect(res.status).toBe(500);
    expect((await env.DB.prepare("SELECT count(*) AS n FROM invites WHERE created_by = ?").bind(user.id).first<{ n: number }>())?.n).toBe(0);
    expect(await audits("invite.create")).toHaveLength(0);
    expect((await create(session)).status).toBe(201);
    expect(await audits("invite.create")).toHaveLength(1);
  });
});

describe("GET /api/invites", () => {
  it("lists only own user invites, newest first, with granted and remaining equal to invitesLeft of /api/me", async () => {
    const user = await createTestUser(env, { invitesGranted: 5 });
    const other = await createTestUser(env);
    const session = await loginAs(user.id);
    const a = await createTestInvite(env, { createdBy: user.id });
    const b = await createTestInvite(env, { createdBy: user.id, revoked: true });
    const c = await createTestInvite(env, { createdBy: user.id, usedBy: other.id });
    await createTestInvite(env, { createdBy: other.id });
    await createTestInvite(env, { kind: "bootstrap" });
    await env.DB.prepare("UPDATE invites SET created_at = ? WHERE id = ?").bind(100, a.id).run();
    await env.DB.prepare("UPDATE invites SET created_at = ? WHERE id = ?").bind(300, b.id).run();
    await env.DB.prepare("UPDATE invites SET created_at = ? WHERE id = ?").bind(200, c.id).run();
    const body = await list(session);
    expect(body.invites.map((i) => i.id)).toEqual([b.id, c.id, a.id]);
    expect(body.invites.find((i) => i.id === c.id)?.usedBy).toBe(other.handle);
    expect(body.invites.find((i) => i.id === b.id)?.revokedAt).not.toBeNull();
    // 5 granted, 3 created, one of them revoked-unused (given back): 5 - 2 = 3.
    expect(body.granted).toBe(5);
    expect(body.remaining).toBe(3);
    const me = MeResponse.parse(await (await authedCall(session, "/api/me")).json());
    expect(me.invitesLeft).toBe(body.remaining);
  });

  it("remaining never goes below 0", async () => {
    const user = await createTestUser(env, { invitesGranted: 1 });
    await createTestInvite(env, { createdBy: user.id });
    await createTestInvite(env, { createdBy: user.id });
    const session = await loginAs(user.id);
    expect((await list(session)).remaining).toBe(0);
  });
});

describe("DELETE /api/invites/:id", () => {
  it("revokes an own unused invite and audits it; the invite can no longer register", async () => {
    const user = await createTestUser(env);
    const session = await loginAs(user.id);
    const invite = await createTestInvite(env, { createdBy: user.id });
    const res = await revoke(session, invite.id);
    expect(res.status).toBe(200);
    expect(await res.json()).toEqual({ ok: true });
    const row = await env.DB.prepare("SELECT revoked_at FROM invites WHERE id = ?").bind(invite.id).first<{ revoked_at: number | null }>();
    expect(row?.revoked_at).not.toBeNull();
    const [event] = await audits("invite.revoke");
    expect(event).toMatchObject({ actor_user_id: user.id, target_id: invite.id });
    const reg = await authedCall({ cookie: "" }, "/api/auth/register/options", { method: "POST", body: JSON.stringify({ invite: invite.code, handle: "revoked-try", displayName: "X" }) });
    expect(reg.status).toBe(400);
  });

  it("a used or already revoked invite is 409 conflict", async () => {
    const user = await createTestUser(env);
    const session = await loginAs(user.id);
    const used = await createTestInvite(env, { createdBy: user.id, usedBy: (await createTestUser(env)).id });
    const done = await createTestInvite(env, { createdBy: user.id, revoked: true });
    for (const invite of [used, done]) {
      const res = await revoke(session, invite.id);
      expect(res.status).toBe(409);
      expect(await errorCode(res)).toBe("conflict");
    }
    expect(await audits("invite.revoke")).toHaveLength(0);
    const row = await env.DB.prepare("SELECT revoked_at FROM invites WHERE id = ?").bind(used.id).first<{ revoked_at: number | null }>();
    expect(row?.revoked_at).toBeNull();
  });

  it("another user's invite, a bootstrap invite and an unknown id are 404", async () => {
    const user = await createTestUser(env);
    const other = await createTestUser(env);
    const session = await loginAs(user.id);
    const foreign = await createTestInvite(env, { createdBy: other.id });
    const bootstrap = await createTestInvite(env, { kind: "bootstrap" });
    for (const id of [foreign.id, bootstrap.id, crypto.randomUUID()]) {
      const res = await revoke(session, id);
      expect(res.status).toBe(404);
      expect(await errorCode(res)).toBe("not_found");
    }
    const row = await env.DB.prepare("SELECT revoked_at FROM invites WHERE id = ?").bind(foreign.id).first<{ revoked_at: number | null }>();
    expect(row?.revoked_at).toBeNull();
  });

  it("revoking works while invites are paused", async () => {
    const user = await createTestUser(env);
    const invite = await createTestInvite(env, { createdBy: user.id });
    await setPaused("1");
    expect((await revoke(await loginAs(user.id), invite.id)).status).toBe(200);
  });
});
