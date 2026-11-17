// A-09 admin API: users, branch block and unblock, invite grants, settings, audit view; admin and fresh on every route.
import { env } from "cloudflare:workers";
import { beforeEach, describe, expect, it } from "vitest";
import { AdminAuditResponse, AdminBlockResponse, AdminSettingsSchema, AdminUsersResponse, ApiErrorSchema, CreateInviteResponse } from "@monoptah/protocol";
import {
  authedCall,
  createTestDesktop,
  createTestInvite,
  createTestUser,
  fakeRoomNamespace,
  loginAs,
  loginWith,
  postJson,
  registerOptions,
  registerUser,
  withRoom,
  withVars,
  type TestSession,
} from "./support/accounts.ts";

beforeEach(async () => {
  await env.DB.prepare("DELETE FROM users").run();
  await env.DB.prepare("DELETE FROM settings").run();
  await env.DB.prepare("DELETE FROM audit_log").run();
});

const errorCode = async (res: Response) => ApiErrorSchema.parse(await res.json()).code;
const json = (body: unknown) => JSON.stringify(body);
const adminSession = async () => {
  const admin = await createTestUser(env, { role: "admin", handle: `adm-${crypto.randomUUID().slice(0, 8)}` });
  return { admin, session: await loginAs(admin.id, { fresh: true }) };
};
const block = (session: TestSession, id: string, body: unknown = { cascade: true, reason: "abuse" }, testEnv?: typeof env) =>
  authedCall(session, `/api/admin/users/${id}/block`, { method: "POST", body: json(body) }, testEnv);
const unblock = (session: TestSession, id: string) => authedCall(session, `/api/admin/users/${id}/unblock`, { method: "POST" });
const userRow = (id: string) =>
  env.DB.prepare("SELECT status, blocked_at, blocked_reason, blocked_via FROM users WHERE id = ?").bind(id).first<{ status: string; blocked_at: number | null; blocked_reason: string | null; blocked_via: string | null }>();
const setInvitedBy = (id: string, inviter: string) => env.DB.prepare("UPDATE users SET invited_by = ? WHERE id = ?").bind(inviter, id).run();
const first = async <T>(sql: string, ...bind: unknown[]) => env.DB.prepare(sql).bind(...bind).first<T>();
const count = async (sql: string, ...bind: unknown[]) => (await first<{ n: number }>(sql, ...bind))?.n ?? 0;
const audits = async (action: string) =>
  (await env.DB.prepare("SELECT actor_user_id, actor_kind, target_id, meta FROM audit_log WHERE action = ?").bind(action).all<{ actor_user_id: string; actor_kind: string; target_id: string; meta: string }>()).results;

interface Seeded {
  id: string;
  session: TestSession;
  desktopId: string;
  tokenId: string;
  deviceId: string;
  unusedInvite: string;
  usedInvite: string;
}

/** A user with a session, a desktop with a token, a device with grants and a push subscription, and two invites. */
async function seedUser(handlePrefix: string): Promise<Seeded> {
  const user = await createTestUser(env, { handle: `${handlePrefix}-${crypto.randomUUID().slice(0, 6)}` });
  const session = await loginAs(user.id);
  const desktopId = await createTestDesktop(env, user.id);
  const tokenId = crypto.randomUUID();
  await env.DB.prepare("INSERT INTO desktop_tokens (id, desktop_id, token_hash, created_at) VALUES (?, ?, ?, 1)").bind(tokenId, desktopId, `th-${tokenId}`).run();
  const deviceId = crypto.randomUUID();
  await env.DB.prepare("INSERT INTO devices (id, user_id, name, public_key, fingerprint, created_at) VALUES (?, ?, 'Phone', 'k', 'f', 1)").bind(deviceId, user.id).run();
  // The grant of its own device on its own desktop, and a grant of its device on someone else's desktop (added by the test).
  await env.DB.prepare("INSERT INTO device_grants (device_id, desktop_id, state, permission, updated_at) VALUES (?, ?, 'confirmed', 'read', 1)").bind(deviceId, desktopId).run();
  await env.DB.prepare("INSERT INTO push_subscriptions (id, user_id, device_id, endpoint, p256dh, auth, vapid_key_id, created_at) VALUES (?, ?, ?, ?, 'p', 'a', 'v', 1)")
    .bind(crypto.randomUUID(), user.id, deviceId, `https://push.example/${crypto.randomUUID()}`)
    .run();
  const unused = await createTestInvite(env, { createdBy: user.id });
  const used = await createTestInvite(env, { createdBy: user.id, usedBy: (await createTestUser(env)).id });
  return { id: user.id, session, desktopId, tokenId, deviceId, unusedInvite: unused.id, usedInvite: used.id };
}

describe("every admin route", () => {
  const routes: [string, string, unknown][] = [
    ["GET", "/api/admin/users", undefined],
    ["POST", "/api/admin/users/someone/block", { cascade: false, reason: "x" }],
    ["POST", "/api/admin/users/someone/unblock", undefined],
    ["POST", "/api/admin/users/someone/invites", { count: 1 }],
    ["GET", "/api/admin/settings", undefined],
    ["PUT", "/api/admin/settings", { invitesPaused: true }],
    ["GET", "/api/admin/audit", undefined],
  ];
  const send = (session: { cookie: string }, method: string, path: string, body: unknown) =>
    authedCall(session, path, { method, ...(body === undefined ? {} : { body: json(body) }) });

  it.each(routes)("%s %s: a non-admin gets 403 forbidden", async (method, path, body) => {
    const user = await createTestUser(env);
    const res = await send(await loginAs(user.id, { fresh: true }), method, path, body);
    expect(res.status).toBe(403);
    expect(await errorCode(res)).toBe("forbidden");
  });

  it.each(routes)("%s %s: an admin with a stale session gets 403 fresh_required", async (method, path, body) => {
    const admin = await createTestUser(env, { role: "admin" });
    const stale = await loginAs(admin.id);
    const old = await loginAs(admin.id, { fresh: true });
    await env.DB.prepare("UPDATE sessions SET reauth_at = ? WHERE id_hash = ?").bind(Math.floor(Date.now() / 1000) - 3600, old.idHash).run();
    for (const session of [stale, old]) {
      const res = await send(session, method, path, body);
      expect(res.status).toBe(403);
      expect(await errorCode(res)).toBe("fresh_required");
    }
  });

  it.each(routes)("%s %s: no session is 401", async (method, path, body) => {
    expect((await send({ cookie: "" }, method, path, body)).status).toBe(401);
  });
});

describe("GET /api/admin/users", () => {
  it("lists every user by creation time with the live desktop count and the block fields", async () => {
    const { admin, session } = await adminSession();
    const a = await createTestUser(env, { handle: "list-a" });
    const b = await createTestUser(env, { handle: "list-b", status: "blocked" });
    await env.DB.prepare("UPDATE users SET created_at = 10 WHERE id = ?").bind(a.id).run();
    await env.DB.prepare("UPDATE users SET created_at = 20, blocked_via = ?, invited_by = ? WHERE id = ?").bind(a.id, a.id, b.id).run();
    await env.DB.prepare("UPDATE users SET created_at = 5, invites_granted = 7 WHERE id = ?").bind(admin.id).run();
    await createTestDesktop(env, a.id);
    await createTestDesktop(env, a.id);
    await createTestDesktop(env, a.id, { revoked: true });
    const res = await authedCall(session, "/api/admin/users");
    expect(res.status).toBe(200);
    const body = AdminUsersResponse.parse(await res.json());
    expect(body.users.map((u) => u.handle)).toEqual([admin.handle, "list-a", "list-b"]);
    expect(body.users[0]).toMatchObject({ role: "admin", invitesGranted: 7, desktopCount: 0, status: "active", invitedBy: null, blockedVia: null });
    expect(body.users[1]).toMatchObject({ id: a.id, desktopCount: 2 });
    expect(body.users[2]).toMatchObject({ id: b.id, status: "blocked", invitedBy: a.id, blockedVia: a.id, desktopCount: 0 });
  });
});

describe("POST /api/admin/users/:id/block", () => {
  async function tree() {
    const { admin, session } = await adminSession();
    const root = await seedUser("root");
    const child = await seedUser("child");
    const grandchild = await seedUser("grand");
    const unrelated = await seedUser("other");
    await setInvitedBy(child.id, root.id);
    await setInvitedBy(grandchild.id, child.id);
    return { admin, session, root, child, grandchild, unrelated };
  }

  it("cascade blocks exactly the branch in one batch and applies every effect", async () => {
    const { admin, session, root, child, grandchild, unrelated } = await tree();
    // The grandchild's device is also granted on the unrelated user's desktop: its grant must be revoked.
    await env.DB.prepare("INSERT INTO device_grants (device_id, desktop_id, state, permission, updated_at) VALUES (?, ?, 'confirmed', 'read', 1)").bind(grandchild.deviceId, unrelated.desktopId).run();
    const fake = fakeRoomNamespace();
    const res = await block(session, root.id, { cascade: true, reason: "Spam ring" }, withRoom(env, fake));
    expect(res.status).toBe(200);
    expect(AdminBlockResponse.parse(await res.json())).toEqual({ blocked: 3 });

    for (const member of [root, child, grandchild]) {
      expect(await userRow(member.id), member.id).toMatchObject({ status: "blocked", blocked_reason: "Spam ring", blocked_via: root.id });
      expect((await userRow(member.id))?.blocked_at).not.toBeNull();
      expect(await count("SELECT count(*) AS n FROM sessions WHERE user_id = ?", member.id), "sessions").toBe(0);
      expect(await first("SELECT 1 FROM desktop_tokens WHERE id = ? AND revoked_at IS NOT NULL", member.tokenId), "token").not.toBeNull();
      expect(await count("SELECT count(*) AS n FROM device_grants WHERE device_id = ? AND state <> 'revoked'", member.deviceId), "grants").toBe(0);
      expect(await count("SELECT count(*) AS n FROM push_subscriptions WHERE user_id = ?", member.id), "push").toBe(0);
      expect(await first("SELECT 1 FROM invites WHERE id = ? AND revoked_at IS NOT NULL", member.unusedInvite), "unused invite revoked").not.toBeNull();
      expect(await first("SELECT 1 FROM invites WHERE id = ? AND revoked_at IS NULL", member.usedInvite), "used invite kept").not.toBeNull();
      // Data is kept for the audit trail.
      expect(await first("SELECT 1 FROM desktops WHERE id = ?", member.desktopId)).not.toBeNull();
      expect(await first("SELECT 1 FROM devices WHERE id = ?", member.deviceId)).not.toBeNull();
    }
    // The unrelated user is untouched, and so is the admin.
    expect(await userRow(unrelated.id)).toMatchObject({ status: "active", blocked_at: null, blocked_reason: null, blocked_via: null });
    expect(await count("SELECT count(*) AS n FROM sessions WHERE user_id = ?", unrelated.id)).toBe(1);
    expect(await first("SELECT 1 FROM desktop_tokens WHERE id = ? AND revoked_at IS NULL", unrelated.tokenId)).not.toBeNull();
    expect(await first("SELECT 1 FROM device_grants WHERE device_id = ? AND desktop_id = ? AND state = 'confirmed'", unrelated.deviceId, unrelated.desktopId)).not.toBeNull();
    expect(await count("SELECT count(*) AS n FROM push_subscriptions WHERE user_id = ?", unrelated.id)).toBe(1);
    expect(await first("SELECT 1 FROM invites WHERE id = ? AND revoked_at IS NULL", unrelated.unusedInvite)).not.toBeNull();
    expect(await userRow(admin.id)).toMatchObject({ status: "active" });
    // The grant of a branch device on someone else's desktop is revoked too.
    expect(await first("SELECT 1 FROM device_grants WHERE device_id = ? AND desktop_id = ? AND state = 'revoked'", grandchild.deviceId, unrelated.desktopId)).not.toBeNull();

    // One audit event for the root, with no reason text.
    const events = await audits("user.block");
    expect(events).toHaveLength(1);
    expect(events[0]).toMatchObject({ actor_user_id: admin.id, actor_kind: "admin", target_id: root.id });
    expect(JSON.parse(events[0]?.meta ?? "")).toEqual({ cascade: true, count: 3 });
    expect(JSON.stringify(events)).not.toContain("Spam ring");

    // The sockets: room.revoke({all: true}, 4011) on each desktop of the branch, none on the unrelated one.
    expect(fake.calls.map((c) => c.desktopId).sort()).toEqual([root.desktopId, child.desktopId, grandchild.desktopId].sort());
    for (const call of fake.calls) expect(call).toMatchObject({ target: { all: true }, code: 4011 });
  });

  it("the next request of a blocked user is refused, and so is a session made after the block", async () => {
    const { session, root, child } = await tree();
    expect((await block(session, root.id)).status).toBe(200);
    for (const member of [root, child]) {
      expect((await authedCall(member.session, "/api/me")).status).toBe(401);
      const later = await loginAs(member.id);
      const res = await authedCall(later, "/api/me");
      expect(res.status).toBe(403);
      expect(await errorCode(res)).toBe("blocked");
    }
  });

  it("a blocked user cannot sign in with a passkey", async () => {
    const { session } = await adminSession();
    const user = await registerUser((await createTestInvite(env, { kind: "bootstrap" })).code);
    expect((await loginWith(user.key)).res.status).toBe(200);
    expect((await block(session, user.userId, { cascade: false, reason: "r" })).status).toBe(200);
    const { res, cookie } = await loginWith(user.key);
    expect(res.status).toBe(403);
    expect(await errorCode(res)).toBe("blocked");
    expect(cookie).toBe("");
  });

  it("cascade false blocks only the target and leaves blocked_via empty", async () => {
    const { session, root, child, grandchild } = await tree();
    const res = await block(session, root.id, { cascade: false, reason: "one" });
    expect(AdminBlockResponse.parse(await res.json())).toEqual({ blocked: 1 });
    expect(await userRow(root.id)).toMatchObject({ status: "blocked", blocked_reason: "one", blocked_via: null });
    expect(await userRow(child.id)).toMatchObject({ status: "active" });
    expect(await userRow(grandchild.id)).toMatchObject({ status: "active" });
    expect(await count("SELECT count(*) AS n FROM sessions WHERE user_id = ?", child.id)).toBe(1);
    expect(JSON.parse((await audits("user.block"))[0]?.meta ?? "")).toEqual({ cascade: false, count: 1 });
  });

  it("a descendant that is already blocked keeps its own reason and blocked_via", async () => {
    const { session, root, child, grandchild } = await tree();
    expect((await block(session, child.id, { cascade: false, reason: "first" })).status).toBe(200);
    const before = await userRow(child.id);
    const res = await block(session, root.id, { cascade: true, reason: "second" });
    expect(AdminBlockResponse.parse(await res.json())).toEqual({ blocked: 2 });
    expect(await userRow(child.id)).toEqual(before);
    expect(await userRow(grandchild.id)).toMatchObject({ blocked_reason: "second", blocked_via: root.id });
  });

  it("never blocks the acting admin when the admin is inside the branch, but blocks the admin's descendants", async () => {
    const { admin, session, root } = await tree();
    const below = await seedUser("below");
    await setInvitedBy(admin.id, root.id);
    await setInvitedBy(below.id, admin.id);
    const res = await block(session, root.id);
    expect(AdminBlockResponse.parse(await res.json())).toEqual({ blocked: 3 + 1 });
    expect(await userRow(admin.id)).toMatchObject({ status: "active", blocked_via: null });
    expect(await count("SELECT count(*) AS n FROM sessions WHERE user_id = ?", admin.id)).toBe(1);
    expect(await userRow(below.id)).toMatchObject({ status: "blocked", blocked_via: root.id });
  });

  it("refuses to block yourself (409) and an unknown id (404) and a bad body (400)", async () => {
    const { admin, session } = await adminSession();
    const self = await block(session, admin.id);
    expect(self.status).toBe(409);
    expect(await errorCode(self)).toBe("conflict");
    expect((await userRow(admin.id))?.status).toBe("active");
    const unknown = await block(session, crypto.randomUUID());
    expect(unknown.status).toBe(404);
    expect(await errorCode(unknown)).toBe("not_found");
    const other = await createTestUser(env);
    expect((await block(session, other.id, { cascade: "yes" })).status).toBe(400);
    expect((await block(session, other.id, { cascade: true, reason: "x".repeat(501) })).status).toBe(400);
    expect(await audits("user.block")).toHaveLength(0);
  });

  it("caps the room.revoke calls and still commits when a room fails", async () => {
    const { session, root } = await tree();
    for (let i = 0; i < 45; i += 1) await createTestDesktop(env, root.id);
    const fake = fakeRoomNamespace();
    const failing = withRoom(env, { namespace: { ...fake.namespace, get: () => ({ revoke: () => Promise.reject(new Error("boom")) }) } as unknown as typeof env.ROOM });
    const res = await block(session, root.id, { cascade: false, reason: "r" }, failing);
    expect(res.status).toBe(200);
    expect((await userRow(root.id))?.status).toBe("blocked");
    const capped = fakeRoomNamespace();
    const other = await seedUser("many");
    for (let i = 0; i < 45; i += 1) await createTestDesktop(env, other.id);
    expect((await block(session, other.id, { cascade: false, reason: "r" }, withRoom(env, capped))).status).toBe(200);
    expect(capped.calls).toHaveLength(40);
  });
});

describe("POST /api/admin/users/:id/unblock", () => {
  it("unblocking the root restores sign-in only: descendants stay blocked, tokens and grants stay revoked", async () => {
    const { admin, session } = await adminSession();
    const root = await seedUser("root");
    const child = await seedUser("child");
    await setInvitedBy(child.id, root.id);
    expect((await block(session, root.id)).status).toBe(200);
    const res = await unblock(session, root.id);
    expect(res.status).toBe(200);
    expect(await res.json()).toEqual({ ok: true });
    expect(await userRow(root.id)).toEqual({ status: "active", blocked_at: null, blocked_reason: null, blocked_via: null });
    expect(await userRow(child.id)).toMatchObject({ status: "blocked", blocked_via: root.id });
    // Never restored: tokens, grants, push subscriptions, invites, sessions.
    expect(await first("SELECT 1 FROM desktop_tokens WHERE id = ? AND revoked_at IS NOT NULL", root.tokenId)).not.toBeNull();
    expect(await first("SELECT 1 FROM device_grants WHERE device_id = ? AND state = 'revoked'", root.deviceId)).not.toBeNull();
    expect(await count("SELECT count(*) AS n FROM push_subscriptions WHERE user_id = ?", root.id)).toBe(0);
    expect(await first("SELECT 1 FROM invites WHERE id = ? AND revoked_at IS NOT NULL", root.unusedInvite)).not.toBeNull();
    expect(await count("SELECT count(*) AS n FROM sessions WHERE user_id = ?", root.id)).toBe(0);
    // The root can sign in again (a new session passes the resolver), the child cannot.
    expect((await authedCall(await loginAs(root.id), "/api/me")).status).toBe(200);
    expect((await authedCall(await loginAs(child.id), "/api/me")).status).toBe(403);
    const events = await audits("user.unblock");
    expect(events).toHaveLength(1);
    expect(events[0]).toMatchObject({ actor_user_id: admin.id, actor_kind: "admin", target_id: root.id });
    // Unblocking the child one by one works, and a user that is not blocked is a conflict.
    expect((await unblock(session, child.id)).status).toBe(200);
    expect((await userRow(child.id))?.status).toBe("active");
    const again = await unblock(session, child.id);
    expect(again.status).toBe(409);
    expect(await errorCode(again)).toBe("conflict");
  });

  it("an unknown id is 404", async () => {
    const { session } = await adminSession();
    const res = await unblock(session, crypto.randomUUID());
    expect(res.status).toBe(404);
    expect(await errorCode(res)).toBe("not_found");
  });
});

describe("POST /api/admin/users/:id/invites", () => {
  const grant = (session: TestSession, id: string, body: unknown) => authedCall(session, `/api/admin/users/${id}/invites`, { method: "POST", body: json(body) });

  it("adds to invites_granted and audits it", async () => {
    const { admin, session } = await adminSession();
    const user = await createTestUser(env, { invitesGranted: 3 });
    const res = await grant(session, user.id, { count: 5 });
    expect(res.status).toBe(200);
    expect((await first<{ invites_granted: number }>("SELECT invites_granted FROM users WHERE id = ?", user.id))?.invites_granted).toBe(8);
    const [event] = await audits("admin.invites_grant");
    expect(event).toMatchObject({ actor_user_id: admin.id, actor_kind: "admin", target_id: user.id });
    expect(JSON.parse(event?.meta ?? "")).toEqual({ count: 5 });
    // The user can now create the extra invites.
    expect((await authedCall(await loginAs(user.id), "/api/me")).status).toBe(200);
  });

  it("refuses a bad count (400), an unknown user (404) and a total above 1000 (409, nothing written)", async () => {
    const { session } = await adminSession();
    const user = await createTestUser(env, { invitesGranted: 950 });
    for (const count of [0, -1, 101, 1.5, "3"]) expect((await grant(session, user.id, { count })).status, String(count)).toBe(400);
    expect((await grant(session, crypto.randomUUID(), { count: 1 })).status).toBe(404);
    const over = await grant(session, user.id, { count: 51 });
    expect(over.status).toBe(409);
    expect(await errorCode(over)).toBe("conflict");
    expect((await first<{ invites_granted: number }>("SELECT invites_granted FROM users WHERE id = ?", user.id))?.invites_granted).toBe(950);
    expect(await audits("admin.invites_grant")).toHaveLength(0);
    expect((await grant(session, user.id, { count: 50 })).status).toBe(200);
  });
});

describe("GET and PUT /api/admin/settings", () => {
  const put = (session: TestSession, body: unknown) => authedCall(session, "/api/admin/settings", { method: "PUT", body: json(body) });
  const get = async (session: TestSession, testEnv?: typeof env) => AdminSettingsSchema.parse(await (await authedCall(session, "/api/admin/settings", {}, testEnv)).json());
  const create = (session: TestSession) => authedCall(session, "/api/invites", { method: "POST" });

  it("GET returns invitesPaused, maxUsers and the user count (blocked included)", async () => {
    const { session } = await adminSession();
    await createTestUser(env, { status: "blocked" });
    expect(await get(session, withVars(env, { MAX_USERS: "0" }))).toEqual({ invitesPaused: false, maxUsers: 0, userCount: 2 });
    expect(await get(session, withVars(env, { MAX_USERS: "2" }))).toEqual({ invitesPaused: false, maxUsers: 2, userCount: 2 });
  });

  it("PUT invitesPaused takes effect on POST /api/invites and registration; login works; false restores; audited", async () => {
    const { admin, session } = await adminSession();
    const existing = await registerUser((await createTestInvite(env, { kind: "bootstrap" })).code);
    const inviter = await createTestUser(env);
    const inviterSession = await loginAs(inviter.id);
    const issuedRes = await create(inviterSession);
    expect(issuedRes.status).toBe(201);
    const issued = CreateInviteResponse.parse(await issuedRes.json());

    const paused = await put(session, { invitesPaused: true });
    expect(paused.status).toBe(200);
    expect(AdminSettingsSchema.parse(await paused.json())).toMatchObject({ invitesPaused: true, maxUsers: Number(env.MAX_USERS) });
    const row = await first<{ value: string; updated_by: string }>("SELECT value, updated_by FROM settings WHERE key = 'invites_paused'");
    expect(row).toEqual({ value: "1", updated_by: admin.id });

    const denied = await create(inviterSession);
    expect(denied.status).toBe(409);
    expect(await errorCode(denied)).toBe("invites_paused");
    const options = await postJson("/api/auth/register/options", { invite: issued.code, handle: `late-${crypto.randomUUID().slice(0, 6)}`, displayName: "X" });
    expect(options.status).toBe(400);
    expect(await errorCode(options)).toBe("invalid_invite");
    expect((await loginWith(existing.key)).res.status).toBe(200);
    expect((await get(session)).invitesPaused).toBe(true);

    expect(AdminSettingsSchema.parse(await (await put(session, { invitesPaused: false })).json()).invitesPaused).toBe(false);
    expect((await create(inviterSession)).status).toBe(201);
    expect((await registerOptions(issued.code, `back-${crypto.randomUUID().slice(0, 6)}`)).options.challenge).toBeTruthy();

    const events = await audits("admin.settings_change");
    expect(events.map((e) => JSON.parse(e.meta))).toEqual([{ invitesPaused: true }, { invitesPaused: false }]);
    for (const event of events) expect(event).toMatchObject({ actor_user_id: admin.id, actor_kind: "admin", target_id: "invites_paused" });
  });

  it("rejects a bad body", async () => {
    const { session } = await adminSession();
    for (const body of [{}, { invitesPaused: "yes" }, null]) expect((await put(session, body)).status).toBe(400);
    expect(await audits("admin.settings_change")).toHaveLength(0);
  });
});

describe("GET /api/admin/audit", () => {
  it("lists every actor's events newest first and pages by the (ts, id) cursor", async () => {
    const { session } = await adminSession();
    const insert = env.DB.prepare("INSERT INTO audit_log (id, ts, actor_user_id, actor_kind, action, target_type, target_id, meta) VALUES (?, ?, ?, ?, ?, 'user', 't', '{\"n\":1}')");
    await env.DB.batch([
      insert.bind("e1", 100, "u1", "user", "invite.create"),
      insert.bind("e2", 200, "u2", "user", "invite.create"),
      insert.bind("e3", 200, null, "system", "system.deploy"),
      insert.bind("e4", 300, "u1", "admin", "user.block"),
      insert.bind("e5", 50, null, "system", "system.deploy"),
    ]);
    const page = async (query = "") => {
      const res = await authedCall(session, `/api/admin/audit${query}`);
      expect(res.status).toBe(200);
      return AdminAuditResponse.parse(await res.json());
    };
    const one = await page("?limit=2");
    expect(one.events.map((e) => e.id)).toEqual(["e4", "e3"]);
    expect(one.events[0]).toMatchObject({ actorUserId: "u1", actorKind: "admin", action: "user.block", targetType: "user", targetId: "t", meta: { n: 1 } });
    expect(one.events[1]).toMatchObject({ actorUserId: null, actorKind: "system" });
    expect(one.nextCursor).not.toBeNull();
    const two = await page(`?limit=2&cursor=${one.nextCursor}`);
    expect(two.events.map((e) => e.id)).toEqual(["e2", "e1"]);
    const three = await page(`?limit=2&cursor=${two.nextCursor}`);
    expect(three.events.map((e) => e.id)).toEqual(["e5"]);
    expect(three.nextCursor).toBeNull();
    expect((await authedCall(session, "/api/admin/audit?cursor=!!!")).status).toBe(400);
    expect((await authedCall(session, "/api/admin/audit?limit=0")).status).toBe(400);
  });
});
