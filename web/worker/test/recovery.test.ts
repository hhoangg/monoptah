// A-05: recovery codes. Sign-in with a code, the recovery session limits, regenerate, the ack, and the
// "never in clear" checks (logs and D1).
import { env } from "cloudflare:workers";
import { afterEach, describe, expect, it, vi } from "vitest";
import { API_ROUTES } from "@monoptah/protocol";
import { RECOVERY_ONLY_ALLOWLIST } from "../src/middleware/auth.ts";
import { ackRecoveryCodes } from "../src/modules/recovery/regenerate.ts";
import { hashRecoveryCode, normalizeRecoveryCode } from "../src/modules/recovery/codes.ts";
import { sha256Hex } from "../src/lib/crypto.ts";
import { authedCall, createTestDesktop, createTestInvite, createTestUser, loginAs, postJson, registerUser, setCookies, withRoom, fakeRoomNamespace, recordingEnv, type RegisteredUser } from "./support/accounts.ts";

const errorCode = async (res: Response) => ((await res.json()) as { code: string }).code;
const recover = (handle: string, code: string, init: Parameters<typeof postJson>[2] = {}) => postJson("/api/auth/recover", { handle, code }, init);
const regenerate = (cookie: string) => authedCall({ cookie }, "/api/recovery-codes/regenerate", { method: "POST" });
const newUser = async (): Promise<RegisteredUser> => registerUser((await createTestInvite(env)).code);
const cookieOf = (res: Response) => setCookies(res)[0]?.split(";")[0] ?? "";

describe("POST /api/auth/recover", () => {
  it("signs in with a code (any case, no hyphens), makes a recovery-only session and revokes the others", async () => {
    const user = await newUser();
    const other = await loginAs(user.userId, { fresh: true });
    const code = user.recoveryCodes[0] ?? "";
    const res = await recover(user.handle, code.toLowerCase().replaceAll("-", ""));
    expect(res.status).toBe(200);
    const body = (await res.json()) as { user: { id: string; handle: string }; recoveryOnly: boolean; codesLeft: number };
    expect(body.user.id).toBe(user.userId);
    expect(body.recoveryOnly).toBe(true);
    expect(body.codesLeft).toBe(9);
    expect(setCookies(res)[0]).toMatch(/^__Host-mc_session=.*HttpOnly.*Secure.*SameSite=Strict.*Max-Age=1800/);

    // The old sessions are gone, the recovery one works and is limited.
    expect((await authedCall({ cookie: user.cookie }, "/api/me")).status).toBe(401);
    expect((await authedCall(other, "/api/me")).status).toBe(401);
    const me = await authedCall({ cookie: cookieOf(res) }, "/api/me");
    expect(me.status).toBe(200);
    expect(((await me.json()) as { recoveryOnly: boolean }).recoveryOnly).toBe(true);
    const session = await env.DB.prepare("SELECT amr, recovery_only, reauth_at FROM sessions WHERE user_id = ?").bind(user.userId).all<{ amr: string; recovery_only: number; reauth_at: number | null }>();
    expect(session.results).toEqual([{ amr: "recovery", recovery_only: 1, reauth_at: null }]);

    const audit = await env.DB.prepare("SELECT actor_user_id, meta FROM audit_log WHERE action = 'recovery.use' AND actor_user_id = ?").bind(user.userId).all<{ meta: string }>();
    expect(audit.results).toHaveLength(1);
    expect(JSON.parse(audit.results[0]?.meta ?? "{}")).toMatchObject({ codesLeft: 9, revokedSessions: 2 });
  });

  it("calls room.revoke({sessionIdHash}, 4001) for every old session on each of the user's desktops", async () => {
    const user = await newUser();
    const second = await loginAs(user.userId, { fresh: true });
    const desktops = [await createTestDesktop(env, user.userId), await createTestDesktop(env, user.userId), await createTestDesktop(env, user.userId)];
    const oldHashes = [await sha256Hex(user.cookie.split("=")[1] ?? ""), second.idHash];
    const room = fakeRoomNamespace();
    const res = await recover(user.handle, user.recoveryCodes[0] ?? "", { testEnv: withRoom(env, room) });
    expect(res.status).toBe(200);
    expect(room.calls).toHaveLength(desktops.length * oldHashes.length);
    for (const desktopId of desktops) {
      const forDesktop = room.calls.filter((call) => call.desktopId === desktopId);
      expect(forDesktop.map((call) => (call.target as { sessionIdHash: string }).sessionIdHash).sort()).toEqual([...oldHashes].sort());
      expect(forDesktop.every((call) => call.code === 4001)).toBe(true);
    }
    // The new recovery session is not revoked.
    const newHash = await sha256Hex(cookieOf(res).split("=")[1] ?? "");
    expect(room.calls.some((call) => (call.target as { sessionIdHash: string }).sessionIdHash === newHash)).toBe(false);
  });

  it("a code works once", async () => {
    const user = await newUser();
    const code = user.recoveryCodes[1] ?? "";
    expect((await recover(user.handle, code)).status).toBe(200);
    const again = await recover(user.handle, code);
    expect(again.status).toBe(401);
    expect(await errorCode(again)).toBe("unauthorized");
    // The next code still works and counts down.
    const next = await recover(user.handle, user.recoveryCodes[2] ?? "");
    expect(((await next.json()) as { codesLeft: number }).codesLeft).toBe(8);
  });

  it("two parallel uses of one code make exactly one session", async () => {
    const user = await newUser();
    const code = user.recoveryCodes[0] ?? "";
    const [a, b] = await Promise.all([recover(user.handle, code), recover(user.handle, code)]);
    expect([a.status, b.status].sort()).toEqual([200, 401]);
    const sessions = await env.DB.prepare("SELECT count(*) AS n FROM sessions WHERE user_id = ?").bind(user.userId).first<{ n: number }>();
    expect(sessions?.n).toBe(1);
    const left = await env.DB.prepare("SELECT count(*) AS n FROM recovery_codes WHERE user_id = ? AND used_at IS NULL").bind(user.userId).first<{ n: number }>();
    expect(left?.n).toBe(9);
  });

  it("leaves desktops, devices, grants and the invite count unchanged", async () => {
    const user = await newUser();
    const desktopId = await createTestDesktop(env, user.userId);
    await env.DB.prepare("INSERT INTO devices (id, user_id, name, public_key, fingerprint, created_at) VALUES (?, ?, 'Phone', 'k', 'f', 1)").bind(`dev-${user.userId}`, user.userId).run();
    const snapshot = async () => ({
      desktops: (await env.DB.prepare("SELECT id, revoked_at FROM desktops WHERE user_id = ?").bind(user.userId).all()).results,
      devices: (await env.DB.prepare("SELECT id, revoked_at FROM devices WHERE user_id = ?").bind(user.userId).all()).results,
      grants: (await env.DB.prepare("SELECT count(*) AS n FROM device_grants").first<{ n: number }>())?.n,
      invites: await env.DB.prepare("SELECT invites_granted FROM users WHERE id = ?").bind(user.userId).first(),
    });
    const before = await snapshot();
    expect(before.desktops).toEqual([{ id: desktopId, revoked_at: null }]);
    expect((await recover(user.handle, user.recoveryCodes[0] ?? "")).status).toBe(200);
    expect(await snapshot()).toEqual(before);
  });

  it("answers an unknown handle, a wrong code, a used code and a blocked user the same way, with the same work", async () => {
    const user = await newUser();
    const used = user.recoveryCodes[0] ?? "";
    await recover(user.handle, used);
    const blocked = await newUser();
    await env.DB.prepare("UPDATE users SET status = 'blocked' WHERE id = ?").bind(blocked.userId).run();

    const attempts = [
      { name: "unknown handle", handle: "nobody-here", code: user.recoveryCodes[3] ?? "" },
      { name: "wrong code", handle: user.handle, code: "AAAA-AAAA-AAAA-AAAA" },
      { name: "used code", handle: user.handle, code: used },
      { name: "blocked user", handle: blocked.handle, code: blocked.recoveryCodes[0] ?? "" },
    ];
    const seen: { status: number; body: string; statements: string[] }[] = [];
    for (const attempt of attempts) {
      const recording = recordingEnv();
      const res = await recover(attempt.handle, attempt.code, { testEnv: recording.testEnv });
      seen.push({ status: res.status, body: await res.text(), statements: recording.statements });
    }
    for (const result of seen) {
      expect(result.status).toBe(401);
      expect(JSON.parse(result.body)).toEqual({ error: "Recovery failed", code: "unauthorized" });
      // Same code path: the same statements in the same order (the HMAC is computed before the one lookup).
      expect(result.statements).toEqual(seen[0]?.statements);
    }
    expect(seen[0]?.statements.filter((sql) => sql.includes("recovery_codes"))).toHaveLength(1);
  });

  it("limits attempts to 5 per handle per hour, known handle or not", async () => {
    const user = await newUser();
    for (let i = 0; i < 5; i += 1) expect((await recover(user.handle, "AAAA-AAAA-AAAA-AAAA")).status).toBe(401);
    // Even the right code is refused now.
    const sixth = await recover(user.handle, user.recoveryCodes[0] ?? "");
    expect(sixth.status).toBe(429);
    expect(await errorCode(sixth)).toBe("rate_limited");
    const unknown = `ghost-${crypto.randomUUID().slice(0, 6)}`;
    for (let i = 0; i < 5; i += 1) expect((await recover(unknown, "AAAA-AAAA-AAAA-AAAA")).status).toBe(401);
    expect((await recover(unknown, "AAAA-AAAA-AAAA-AAAA")).status).toBe(429);
  });

  it("limits attempts to 20 per IP per hour", async () => {
    const ip = "198.51.100.77";
    for (let i = 0; i < 20; i += 1) expect((await recover(`ip-user-${i}`, "AAAA-AAAA-AAAA-AAAA", { ip })).status).toBe(401);
    expect((await recover("ip-user-last", "AAAA-AAAA-AAAA-AAAA", { ip })).status).toBe(429);
  });

  it("rejects a bad body with 400", async () => {
    expect((await postJson("/api/auth/recover", { handle: "x" })).status).toBe(400);
    expect((await postJson("/api/auth/recover", { handle: "bob-1", code: "" })).status).toBe(400);
  });
});

describe("recovery-only session limits", () => {
  it("every cookie route outside the add-passkey routes, GET /api/me and logout answers 403 recovery_only", async () => {
    const user = await createTestUser(env, { role: "admin" });
    const session = await loginAs(user.id, { recoveryOnly: true });
    const limited = API_ROUTES.filter(
      (r) =>
        (r.auth === "session" || r.auth === "fresh" || r.auth === "admin" || r.auth === "session+grant") &&
        r.allowRecoveryOnly !== true &&
        !RECOVERY_ONLY_ALLOWLIST.has(`${r.method} ${r.path}`) &&
        r.path.startsWith("/api/"),
    );
    expect(limited.length).toBeGreaterThan(20);
    for (const route of limited) {
      const res = await authedCall(session, route.path.replace(/:[A-Za-z]+/g, "x"), { method: route.method });
      expect(res.status, `${route.method} ${route.path}`).toBe(403);
      expect(await errorCode(res), `${route.method} ${route.path}`).toBe("recovery_only");
    }
    // The allowed ones pass the resolver.
    expect((await authedCall(session, "/api/me")).status).toBe(200);
    expect((await authedCall(session, "/api/passkeys/options", { method: "POST" })).status).toBe(200);
    expect((await authedCall(session, "/api/auth/logout", { method: "POST" })).status).toBe(200);
  });
});

describe("POST /api/recovery-codes/regenerate", () => {
  it("needs a fresh session", async () => {
    const user = await newUser();
    await env.DB.prepare("UPDATE sessions SET reauth_at = ? WHERE user_id = ?").bind(1, user.userId).run();
    const res = await regenerate(user.cookie);
    expect(res.status).toBe(403);
    expect(await errorCode(res)).toBe("fresh_required");
  });

  it("returns 10 new codes, kills the whole old batch and clears the ack", async () => {
    const user = await newUser();
    await ackRecoveryCodes(env, user.userId);
    // One old code is already used: regenerate removes used ones too.
    await recover(user.handle, user.recoveryCodes[0] ?? "");
    const session = await loginAs(user.userId, { fresh: true });

    const res = await authedCall(session, "/api/recovery-codes/regenerate", { method: "POST" });
    expect(res.status).toBe(200);
    const { recoveryCodes } = (await res.json()) as { recoveryCodes: string[] };
    expect(recoveryCodes).toHaveLength(10);
    expect(new Set(recoveryCodes).size).toBe(10);
    for (const code of recoveryCodes) expect(code).toMatch(/^[0-9A-HJKMNP-TV-Z]{4}(-[0-9A-HJKMNP-TV-Z]{4}){3}$/);

    const rows = await env.DB.prepare("SELECT batch, used_at FROM recovery_codes WHERE user_id = ?").bind(user.userId).all<{ batch: number; used_at: number | null }>();
    expect(rows.results).toHaveLength(10);
    expect(rows.results.every((r) => r.batch === 2 && r.used_at === null)).toBe(true);
    const ack = await env.DB.prepare("SELECT recovery_ack_at FROM users WHERE id = ?").bind(user.userId).first<{ recovery_ack_at: number | null }>();
    expect(ack?.recovery_ack_at).toBeNull();
    const audit = await env.DB.prepare("SELECT meta FROM audit_log WHERE action = 'recovery.regenerate' AND actor_user_id = ?").bind(user.userId).all<{ meta: string }>();
    expect(audit.results.map((r) => JSON.parse(r.meta))).toEqual([{ batch: 2 }]);
  });

  it("an old code stops working and a new one works", async () => {
    const user = await newUser();
    const { recoveryCodes } = (await (await regenerate(user.cookie)).json()) as { recoveryCodes: string[] };
    expect((await recover(user.handle, user.recoveryCodes[4] ?? "")).status).toBe(401);
    expect((await recover(user.handle, recoveryCodes[0] ?? "")).status).toBe(200);
  });

  it("is one batch: a failure leaves the old codes untouched", async () => {
    const user = await newUser();
    const ok = recordingEnv();
    expect((await authedCall({ cookie: user.cookie }, "/api/recovery-codes/regenerate", { method: "POST" }, ok.testEnv)).status).toBe(200);
    expect(ok.batches()).toBe(1);

    const other = await newUser();
    const failing = recordingEnv({ failBatch: true });
    const res = await authedCall({ cookie: other.cookie }, "/api/recovery-codes/regenerate", { method: "POST" }, failing.testEnv);
    expect(res.status).toBe(500);
    const rows = await env.DB.prepare("SELECT batch FROM recovery_codes WHERE user_id = ?").bind(other.userId).all<{ batch: number }>();
    expect(rows.results).toHaveLength(10);
    expect(rows.results.every((r) => r.batch === 1)).toBe(true);
    expect((await recover(other.handle, other.recoveryCodes[0] ?? "")).status).toBe(200);
  });

  it("two parallel regenerates leave exactly one live batch", async () => {
    const user = await newUser();
    const [a, b] = await Promise.all([regenerate(user.cookie), regenerate(user.cookie)]);
    expect([a.status, b.status].sort()).toEqual([200, 409]);
    const rows = await env.DB.prepare("SELECT batch FROM recovery_codes WHERE user_id = ?").bind(user.userId).all<{ batch: number }>();
    expect(rows.results).toHaveLength(10);
    expect(new Set(rows.results.map((r) => r.batch)).size).toBe(1);
  });

  it("a recovery-only session cannot regenerate", async () => {
    const user = await newUser();
    const session = await loginAs(user.userId, { recoveryOnly: true });
    const res = await authedCall(session, "/api/recovery-codes/regenerate", { method: "POST" });
    expect(res.status).toBe(403);
    expect(await errorCode(res)).toBe("recovery_only");
  });
});

describe("recovery_ack_at", () => {
  const ackOf = async (cookie: string) => ((await (await authedCall({ cookie }, "/api/me")).json()) as { recoveryAckAt: number | null }).recoveryAckAt;

  it("is null after registration and after regenerate, and set after ackRecoveryCodes", async () => {
    const user = await newUser();
    expect(await ackOf(user.cookie)).toBeNull();
    await ackRecoveryCodes(env, user.userId, 1234);
    expect(await ackOf(user.cookie)).toBe(1234);
    expect((await regenerate(user.cookie)).status).toBe(200);
    expect(await ackOf(user.cookie)).toBeNull();
    await ackRecoveryCodes(env, user.userId);
    expect(await ackOf(user.cookie)).toBeGreaterThan(1234);
  });
});

describe("codes are never logged or stored in clear", () => {
  afterEach(() => vi.restoreAllMocks());

  const variants = (code: string): string[] => {
    const compact = normalizeRecoveryCode(code);
    return [code, code.toLowerCase(), compact, compact.toLowerCase()];
  };

  it("no log line and no D1 row holds a code", async () => {
    const lines: string[] = [];
    for (const method of ["log", "info", "warn", "error", "debug"] as const) {
      vi.spyOn(console, method).mockImplementation((...args: unknown[]) => {
        lines.push(args.map((a) => (typeof a === "string" ? a : JSON.stringify(a))).join(" "));
      });
    }
    const user = await newUser();
    const codes = [...user.recoveryCodes];
    expect((await recover(user.handle, user.recoveryCodes[0] ?? "")).status).toBe(200);
    const session = await loginAs(user.userId, { fresh: true });
    const regenerated = (await (await authedCall(session, "/api/recovery-codes/regenerate", { method: "POST" })).json()) as { recoveryCodes: string[] };
    codes.push(...regenerated.recoveryCodes);
    expect((await recover(user.handle, "AAAA-AAAA-AAAA-AAAA")).status).toBe(401);
    expect((await recover("nobody-here", codes[5] ?? "")).status).toBe(401);
    expect(codes).toHaveLength(20);

    const haystack = lines.join("\n");
    for (const code of codes) for (const form of variants(code)) expect(haystack).not.toContain(form);

    for (const table of ["recovery_codes", "audit_log", "rate_limits", "challenges", "sessions"]) {
      const { results } = await env.DB.prepare(`SELECT * FROM ${table}`).all();
      const dump = JSON.stringify(results).toLowerCase();
      for (const code of codes) {
        expect(dump, table).not.toContain(normalizeRecoveryCode(code).toLowerCase());
        expect(dump, table).not.toContain(code.toLowerCase());
      }
    }
    // The stored value is the HMAC of the code.
    const stored = await env.DB.prepare("SELECT 1 AS ok FROM recovery_codes WHERE code_hash = ?").bind(await hashRecoveryCode(env, regenerated.recoveryCodes[0] ?? "")).first();
    expect(stored).not.toBeNull();
  });
});
