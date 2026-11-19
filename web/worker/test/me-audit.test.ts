import { env } from "cloudflare:workers";
import { describe, expect, it } from "vitest";
import { ApiErrorSchema, AuditResponse, MeResponse, PatchMeResponse } from "@monoptah/protocol";
import { decodeCursor, encodeCursor } from "../src/modules/audit/handlers.ts";
import { writeAudit } from "../src/modules/audit/audit.ts";
import { authedCall, createTestUser, loginAs } from "./support/accounts.ts";

async function insertInvite(createdBy: string, state: "active" | "used" | "revoked" | "revoked-used") {
  const id = crypto.randomUUID();
  const used = state === "used" || state === "revoked-used";
  const revoked = state === "revoked" || state === "revoked-used";
  const invitee = used ? (await createTestUser(env)).id : null;
  await env.DB.prepare("INSERT INTO invites (id, code_hash, created_by, expires_at, used_by, used_at, revoked_at, created_at) VALUES (?, ?, ?, 9999999999, ?, ?, ?, 1)")
    .bind(id, `hash-${id}`, createdBy, invitee, used ? 5 : null, revoked ? 6 : null)
    .run();
}

describe("GET /api/me", () => {
  it("returns the user, invitesLeft, recoveryOnly and recoveryAckAt", async () => {
    const user = await createTestUser(env, { handle: "me-reader", displayName: "Reader", role: "admin", invitesGranted: 5, recoveryAckAt: 1234 });
    const res = await authedCall(await loginAs(user.id), "/api/me");
    expect(res.status).toBe(200);
    expect(MeResponse.parse(await res.json())).toEqual({
      user: { id: user.id, handle: "me-reader", displayName: "Reader", role: "admin" },
      invitesLeft: 5,
      recoveryOnly: false,
      recoveryCodesLeft: 0,
      recoveryAckAt: 1234,
    });
  });

  it("invitesLeft = granted minus invites that are not (revoked and unused)", async () => {
    const user = await createTestUser(env, { invitesGranted: 5 });
    await insertInvite(user.id, "active"); // counted
    await insertInvite(user.id, "used"); // counted
    await insertInvite(user.id, "revoked"); // given back
    await insertInvite(user.id, "revoked-used"); // used, so still counted
    const other = await createTestUser(env);
    await insertInvite(other.id, "active"); // someone else's
    const body = MeResponse.parse(await (await authedCall(await loginAs(user.id), "/api/me")).json());
    expect(body.invitesLeft).toBe(2);
    await env.DB.prepare("UPDATE users SET invites_granted = 1 WHERE id = ?").bind(user.id).run();
    expect(MeResponse.parse(await (await authedCall(await loginAs(user.id), "/api/me")).json()).invitesLeft).toBe(0);
  });

  it("reports recoveryOnly and a null recoveryAckAt for a recovery session", async () => {
    const user = await createTestUser(env);
    const body = MeResponse.parse(await (await authedCall(await loginAs(user.id, { recoveryOnly: true }), "/api/me")).json());
    expect(body.recoveryOnly).toBe(true);
    expect(body.recoveryAckAt).toBeNull();
  });
});

describe("PATCH /api/me", () => {
  it("changes the display name only", async () => {
    const user = await createTestUser(env, { handle: "patch-me" });
    const session = await loginAs(user.id);
    const res = await authedCall(session, "/api/me", { method: "PATCH", body: JSON.stringify({ displayName: "  New Name  ", handle: "hijack", role: "admin" }) });
    expect(res.status).toBe(200);
    expect(PatchMeResponse.parse(await res.json()).user).toEqual({ id: user.id, handle: "patch-me", displayName: "New Name", role: "user" });
    const row = await env.DB.prepare("SELECT handle, display_name, role FROM users WHERE id = ?").bind(user.id).first();
    expect(row).toEqual({ handle: "patch-me", display_name: "New Name", role: "user" });
    const audits = await env.DB.prepare("SELECT count(*) AS n FROM audit_log WHERE actor_user_id = ?").bind(user.id).first<{ n: number }>();
    expect(audits?.n).toBe(0);
  });

  it.each([["empty", { displayName: "" }], ["blank", { displayName: "   " }], ["too long", { displayName: "x".repeat(65) }], ["missing", {}], ["wrong type", { displayName: 5 }]])("rejects a %s display name with 400", async (_n, body) => {
    const session = await loginAs((await createTestUser(env)).id);
    const res = await authedCall(session, "/api/me", { method: "PATCH", body: JSON.stringify(body) });
    expect(res.status).toBe(400);
    expect(ApiErrorSchema.parse(await res.json()).code).toBe("invalid");
  });

  it("rejects a body that is not JSON with 400", async () => {
    const session = await loginAs((await createTestUser(env)).id);
    expect((await authedCall(session, "/api/me", { method: "PATCH", body: "{" })).status).toBe(400);
  });

  it("DELETE /api/me stays 501 (ticket U-11)", async () => {
    const session = await loginAs((await createTestUser(env)).id, { fresh: true });
    expect((await authedCall(session, "/api/me", { method: "DELETE" })).status).toBe(501);
  });
});

describe("GET /api/audit", () => {
  async function seed(userId: string, count: number, ts = 1_900_000_000) {
    for (let i = 0; i < count; i += 1) {
      // Same ts for every event: paging must tie-break on id.
      await writeAudit(env, { action: "passkey.rename", actorUserId: userId, targetType: "passkey", targetId: `p${i}`, meta: { passkeyId: `p${i}` }, ts });
    }
  }

  it("returns only the caller's own events, newest first, with the meta", async () => {
    const user = await createTestUser(env);
    const other = await createTestUser(env);
    await writeAudit(env, { action: "passkey.add", actorUserId: user.id, targetType: "passkey", targetId: "old", meta: { passkeyId: "old" }, ts: 1_000 });
    await writeAudit(env, { action: "passkey.delete", actorUserId: user.id, targetType: "passkey", targetId: "new", meta: { passkeyId: "new", remaining: 1 }, ts: 2_000 });
    await writeAudit(env, { action: "passkey.add", actorUserId: other.id, meta: { passkeyId: "theirs" }, ts: 3_000 });
    await writeAudit(env, { action: "ratelimit.breach", meta: { group: "login", limit: 1, windowSeconds: 1 }, ts: 4_000 });
    const res = await authedCall(await loginAs(user.id), "/api/audit");
    expect(res.status).toBe(200);
    const body = AuditResponse.parse(await res.json());
    expect(body.nextCursor).toBeNull();
    expect(body.events.map((e) => [e.action, e.ts, e.targetId])).toEqual([["passkey.delete", 2_000, "new"], ["passkey.add", 1_000, "old"]]);
    expect(body.events[0]?.meta).toEqual({ passkeyId: "new", remaining: 1 });
    expect(JSON.stringify(body)).not.toContain("theirs");
  });

  it("pages with an opaque cursor over (ts, id), without gaps or repeats, across equal timestamps", async () => {
    const user = await createTestUser(env);
    await seed(user.id, 7, 1_900_000_000);
    await seed(user.id, 2, 1_900_000_100);
    const session = await loginAs(user.id);
    const seen: string[] = [];
    let cursor: string | null = null;
    let pages = 0;
    do {
      const res = await authedCall(session, `/api/audit?limit=4${cursor ? `&cursor=${cursor}` : ""}`);
      expect(res.status).toBe(200);
      const body = AuditResponse.parse(await res.json());
      expect(body.events.length).toBeLessThanOrEqual(4);
      seen.push(...body.events.map((e) => e.id));
      cursor = body.nextCursor;
      pages += 1;
    } while (cursor && pages < 10);
    expect(pages).toBe(3);
    expect(seen).toHaveLength(9);
    expect(new Set(seen).size).toBe(9);
    // Newest first: the two later events come first.
    const all = AuditResponse.parse(await (await authedCall(session, "/api/audit?limit=100")).json());
    expect(all.events.map((e) => e.id)).toEqual(seen);
    expect(all.events.slice(0, 2).every((e) => e.ts === 1_900_000_100)).toBe(true);
    const keys = all.events.map((e) => [e.ts, e.id] as const);
    for (let i = 1; i < keys.length; i += 1) {
      const [pt, pid] = keys[i - 1] as readonly [number, string];
      const [t, id] = keys[i] as readonly [number, string];
      expect(t < pt || (t === pt && id < pid)).toBe(true);
    }
  });

  it("defaults to a page of 50 and ends with a null cursor on an exact fit", async () => {
    const user = await createTestUser(env);
    await seed(user.id, 51);
    const session = await loginAs(user.id);
    const first = AuditResponse.parse(await (await authedCall(session, "/api/audit")).json());
    expect(first.events).toHaveLength(50);
    expect(first.nextCursor).not.toBeNull();
    const second = AuditResponse.parse(await (await authedCall(session, `/api/audit?cursor=${first.nextCursor}`)).json());
    expect(second.events).toHaveLength(1);
    expect(second.nextCursor).toBeNull();
    const exact = AuditResponse.parse(await (await authedCall(session, "/api/audit?limit=51")).json());
    expect(exact.nextCursor).toBeNull();
  });

  it.each(["limit=0", "limit=101", "limit=abc", "cursor=%%%", "cursor=bm90LWEtY3Vyc29y", "cursor=" + encodeURIComponent(btoa("1:id with space"))])("rejects the bad query %s with 400", async (query) => {
    const session = await loginAs((await createTestUser(env)).id);
    const res = await authedCall(session, `/api/audit?${query}`);
    expect(res.status).toBe(400);
    expect(ApiErrorSchema.parse(await res.json()).code).toBe("invalid");
  });

  it("cursor encoding round-trips and rejects junk", () => {
    expect(decodeCursor(encodeCursor(1_900_000_000, "abc-123"))).toEqual({ ts: 1_900_000_000, id: "abc-123" });
    expect(decodeCursor("")).toBeNull();
    expect(decodeCursor("!!")).toBeNull();
    expect(decodeCursor(encodeCursor(5, "a b"))).toBeNull();
  });

  it("needs a session", async () => {
    expect((await authedCall({ cookie: "" }, "/api/audit")).status).toBe(401);
  });
});
