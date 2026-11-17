import { env } from "cloudflare:workers";
import { describe, expect, it } from "vitest";
import { z } from "zod";
import { AUDIT_ACTION_NAMES, AUDIT_ACTIONS, assertSafeMetaKeys, FORBIDDEN_META_KEYS, type AuditAction } from "../src/modules/audit/actions.ts";
import { auditStatement, writeAudit, type AuditEvent } from "../src/modules/audit/audit.ts";
import { clientIp, identifierHash, ipHash } from "../src/lib/ip.ts";
import { hmacSha256Hex, randomToken, sha256Hex, timingSafeEqualBytes, timingSafeEqualStrings } from "../src/lib/crypto.ts";

const HASH = "a".repeat(64);

/** One valid meta per action. The test below fails when an action is added without a sample. */
const SAMPLES: { [A in AuditAction]: Record<string, unknown> } = {
  "account.register": { inviteId: "inv-1", ipHash: HASH },
  "account.login": { passkeyId: "cred_1-A", ipHash: HASH },
  "account.login_refused_counter": { passkeyId: "cred_1-A", ipHash: HASH },
  "passkey.add": { passkeyId: "cred-1", count: 2 },
  "passkey.rename": { passkeyId: "cred-1" },
  "passkey.delete": { passkeyId: "cred-1", remaining: 1 },
  "recovery.use": { codesLeft: 9, revokedSessions: 2, ipHash: HASH },
  "recovery.regenerate": { batch: 2 },
  "invite.create": { inviteId: "inv-1" },
  "invite.use": { inviteId: "inv-1" },
  "invite.revoke": { inviteId: "inv-1", count: 1 },
  "pairing.approve": { desktopId: "desk-1", replaced: false },
  "desktop.create": { desktopId: "desk-1" },
  "desktop.revoke": { desktopId: "desk-1" },
  "device.create": { deviceId: "dev-1" },
  "device.revoke": { deviceId: "dev-1", grants: 3 },
  "grant.change": { deviceId: "dev-1", desktopId: "desk-1", state: "revoked", permission: "read", requestedPermission: "control" },
  "user.block": { cascade: true, count: 4 },
  "user.unblock": { cascade: false },
  "admin.settings_change": { invitesPaused: true },
  "admin.invites_grant": { count: 2 },
  "session.logout": {},
  "session.revoke": { all: true, count: 3 },
  "ratelimit.breach": { group: "login", limit: 30, windowSeconds: 600, ipHash: HASH },
  "system.deploy": { version: "1.2.3+abc" },
};

const SECURITY_MD_10_EVENTS = [
  // register, login, login refused for a counter decrease
  "account.register", "account.login", "account.login_refused_counter",
  // passkey changes
  "passkey.add", "passkey.rename", "passkey.delete",
  // recovery use and regenerate
  "recovery.use", "recovery.regenerate",
  // invite create/use/revoke
  "invite.create", "invite.use", "invite.revoke",
  // pairing approve, desktop/device create and revoke, grant changes
  "pairing.approve", "desktop.create", "desktop.revoke", "device.create", "device.revoke", "grant.change",
  // block/unblock, admin actions (settings, grant invites)
  "user.block", "user.unblock", "admin.settings_change", "admin.invites_grant",
  // rate-limit breach, session revoke and logout, deploy
  "ratelimit.breach", "session.revoke", "session.logout", "system.deploy",
];

describe("audit action registry", () => {
  it("lists every event of security.md 10", () => {
    expect([...AUDIT_ACTION_NAMES].sort()).toEqual([...SECURITY_MD_10_EVENTS].sort());
  });

  it("has a sample for every action", () => {
    expect(Object.keys(SAMPLES).sort()).toEqual([...AUDIT_ACTION_NAMES].sort());
  });

  it.each(AUDIT_ACTION_NAMES)("%s: accepts its sample and rejects an unknown key", (action) => {
    const schema = AUDIT_ACTIONS[action].meta;
    expect(schema.safeParse(SAMPLES[action]).success).toBe(true);
    expect(schema.safeParse({ ...SAMPLES[action], extra: "x" }).success).toBe(false);
    for (const forbidden of ["code", "token", "secret", "cookie", "handle", "body"]) {
      expect(schema.safeParse({ ...SAMPLES[action], [forbidden]: "x" }).success, forbidden).toBe(false);
    }
  });

  it.each(AUDIT_ACTION_NAMES)("%s: has no forbidden key and only id, count, boolean or enum values", (action) => {
    const schema = AUDIT_ACTIONS[action].meta;
    for (const [key, field] of Object.entries(schema.shape)) {
      expect(FORBIDDEN_META_KEYS, `${action}.${key}`).not.toContain(key.toLowerCase());
      const json = JSON.stringify(z.toJSONSchema(field as z.ZodType));
      // No free text: a string must be an enum, a pattern or a bounded id.
      if (json.includes('"type":"string"')) expect(json, `${action}.${key}`).toMatch(/"pattern"|"enum"/);
    }
  });

  it("the forbidden-key guard cannot be bypassed by adding a key to a schema", () => {
    for (const key of ["code", "token", "secret", "cookie", "pepper", "challenge", "password", "body", "content", "handle", "Token", "displayName"]) {
      expect(() => assertSafeMetaKeys("test.action", z.strictObject({ [key]: z.string() })), key).toThrow(/forbidden/);
    }
    expect(() => assertSafeMetaKeys("test.action", z.strictObject({ codesLeft: z.number(), ipHash: z.string() }))).not.toThrow();
  });

  it("every action allows at least one actor kind and uses the schema's actor kinds only", () => {
    for (const action of AUDIT_ACTION_NAMES) expect(AUDIT_ACTIONS[action].actorKinds.length).toBeGreaterThan(0);
  });
});

describe("auditStatement and writeAudit", () => {
  it("writes a row with the validated meta, the actor and the target", async () => {
    const ts = 1_900_000_000;
    await writeAudit(env, { action: "passkey.delete", actorUserId: "user-audit-1", targetType: "passkey", targetId: "cred-9", meta: { passkeyId: "cred-9", remaining: 2 }, ts });
    const row = await env.DB.prepare("SELECT * FROM audit_log WHERE actor_user_id = 'user-audit-1'").first<Record<string, unknown>>();
    expect(row).toMatchObject({ ts, actor_user_id: "user-audit-1", actor_kind: "user", action: "passkey.delete", target_type: "passkey", target_id: "cred-9" });
    expect(JSON.parse(row?.meta as string)).toEqual({ passkeyId: "cred-9", remaining: 2 });
    expect(typeof row?.id).toBe("string");
  });

  it("defaults the actor kind to the first one of the action, and the time to now", async () => {
    const before = Math.floor(Date.now() / 1000);
    await writeAudit(env, { action: "ratelimit.breach", meta: { group: "login", limit: 30, windowSeconds: 600 } });
    const row = await env.DB.prepare("SELECT ts, actor_kind, actor_user_id FROM audit_log WHERE action = 'ratelimit.breach' AND json_extract(meta, '$.windowSeconds') = 600 AND json_extract(meta, '$.ipHash') IS NULL").first<{ ts: number; actor_kind: string; actor_user_id: string | null }>();
    expect(row?.actor_kind).toBe("system");
    expect(row?.actor_user_id).toBeNull();
    expect(row?.ts).toBeGreaterThanOrEqual(before);
  });

  it("returns a statement that can go into a batch with other statements", async () => {
    const statements = [
      env.DB.prepare("INSERT INTO settings (key, value, updated_at) VALUES ('audit-batch', '1', 1) ON CONFLICT (key) DO UPDATE SET value = '1'"),
      auditStatement(env, { action: "admin.settings_change", actorKind: "admin", actorUserId: "admin-batch", meta: { invitesPaused: true } }),
    ];
    const results = await env.DB.batch(statements);
    expect(results).toHaveLength(2);
    const row = await env.DB.prepare("SELECT count(*) AS n FROM audit_log WHERE actor_user_id = 'admin-batch'").first<{ n: number }>();
    expect(row?.n).toBe(1);
  });

  it("throws on meta with an unknown or forbidden key, a wrong type or a missing field", () => {
    const bad = (event: unknown) => () => auditStatement(env, event as AuditEvent);
    expect(bad({ action: "passkey.delete", meta: { passkeyId: "p", remaining: 1, token: "x" } })).toThrow(/Invalid meta/);
    expect(bad({ action: "passkey.delete", meta: { passkeyId: "p" } })).toThrow(/Invalid meta/);
    expect(bad({ action: "passkey.delete", meta: { passkeyId: "p", remaining: "1" } })).toThrow(/Invalid meta/);
    expect(bad({ action: "ratelimit.breach", meta: { group: "login", limit: 1, windowSeconds: 1, ipHash: "203.0.113.5" } })).toThrow(/Invalid meta/);
    expect(bad({ action: "invite.create", meta: { inviteId: "has space and a secret" } })).toThrow(/Invalid meta/);
  });

  it("does not echo the offending value in the error", () => {
    try {
      auditStatement(env, { action: "invite.create", meta: { inviteId: "SECRET-VALUE with spaces" } });
      expect.unreachable();
    } catch (err) {
      expect(String(err)).not.toContain("SECRET-VALUE");
    }
  });

  it("throws on an unknown action and on an actor kind the action does not allow", () => {
    expect(() => auditStatement(env, { action: "made.up", meta: {} } as unknown as AuditEvent)).toThrow(/Unknown audit action/);
    expect(() => auditStatement(env, { action: "user.block", actorKind: "desktop", meta: { cascade: false, count: 1 } })).toThrow(/actor kind/);
  });

  it("records one real event of every action and every stored row validates against its schema", async () => {
    const actor = "user-audit-all";
    for (const action of AUDIT_ACTION_NAMES) {
      await writeAudit(env, { action, actorUserId: actor, meta: SAMPLES[action] } as AuditEvent);
    }
    const { results } = await env.DB.prepare("SELECT action, meta FROM audit_log WHERE actor_user_id = ?").bind(actor).all<{ action: AuditAction; meta: string }>();
    expect(results).toHaveLength(AUDIT_ACTION_NAMES.length);
    for (const row of results) {
      const parsed = AUDIT_ACTIONS[row.action].meta.safeParse(JSON.parse(row.meta));
      expect(parsed.success, row.action).toBe(true);
    }
  });
});

describe("lib helpers", () => {
  it("randomToken is base64url of the requested size and unique", () => {
    const a = randomToken(32);
    expect(a).toMatch(/^[A-Za-z0-9_-]{43}$/);
    expect(randomToken(32)).not.toBe(a);
    expect(randomToken(16)).toMatch(/^[A-Za-z0-9_-]{22}$/);
  });

  it("sha256Hex and hmacSha256Hex match known vectors", async () => {
    expect(await sha256Hex("abc")).toBe("ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad");
    // RFC 4231 test case 2.
    expect(await hmacSha256Hex("Jefe", "what do ya want for nothing?")).toBe("5bdcc146bf60754e6a042426089575c75a003f089d2739839dec58b964ec3843");
  });

  it("constant-time comparisons", () => {
    expect(timingSafeEqualStrings("abc", "abc")).toBe(true);
    expect(timingSafeEqualStrings("abc", "abd")).toBe(false);
    expect(timingSafeEqualStrings("abc", "abcd")).toBe(false);
    expect(timingSafeEqualBytes(new Uint8Array([1, 2]), new Uint8Array([1, 2]))).toBe(true);
    expect(timingSafeEqualBytes(new Uint8Array([1, 2]), new Uint8Array([1, 3]))).toBe(false);
    expect(timingSafeEqualBytes(new Uint8Array([]), new Uint8Array([]))).toBe(true);
  });

  it("clientIp reads CF-Connecting-IP and falls back to unknown", () => {
    expect(clientIp(new Request("http://x/", { headers: { "CF-Connecting-IP": "198.51.100.7" } }))).toBe("198.51.100.7");
    expect(clientIp(new Request("http://x/"))).toBe("unknown");
  });

  it("ipHash is the salted HMAC and identifierHash is namespaced", async () => {
    expect(await ipHash(env, "198.51.100.7")).toBe(await hmacSha256Hex(env.AUDIT_IP_SALT, "198.51.100.7"));
    expect(await ipHash(env, "198.51.100.7")).toMatch(/^[0-9a-f]{64}$/);
    expect(await ipHash({ AUDIT_IP_SALT: "other-salt" }, "198.51.100.7")).not.toBe(await ipHash(env, "198.51.100.7"));
    expect(await identifierHash(env, "invite", "x")).not.toBe(await identifierHash(env, "handle", "x"));
  });
});
