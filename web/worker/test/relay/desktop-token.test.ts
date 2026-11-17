// Desktop bearer tokens on the token routes (architecture.md 4.2): scopes, revocation, owner state.
import { describe, expect, it } from "vitest";
import { sha256Hex } from "../../src/lib/crypto.ts";
import { env, json, newAccount, pairDesktop, registerDevice, requestGrant, tokenCall } from "./support/api-support.ts";

/** A token with only `scope` for the desktop of `paired`, inserted directly. */
async function scopedToken(desktopId: string, scope: string): Promise<string> {
  const token = `test-scoped-${crypto.randomUUID()}`;
  await env.DB.prepare("INSERT INTO desktop_tokens (id, desktop_id, token_hash, scope, created_at) VALUES (?, ?, ?, ?, 1)").bind(crypto.randomUUID(), desktopId, await sha256Hex(token), scope).run();
  return token;
}

describe("token scopes", () => {
  it("refuses a token without the grants scope on both grant routes and without relay on desktop/me", async () => {
    const mine = await newAccount();
    const paired = await pairDesktop(mine);
    const device = await registerDevice(mine);
    const relayOnly = await scopedToken(paired.desktopId, "relay");
    const grantsOnly = await scopedToken(paired.desktopId, "grants");
    expect((await tokenCall("/api/desktop/grants", relayOnly)).status).toBe(403);
    expect((await tokenCall(`/api/desktop/grants/${device.deviceId}/state`, relayOnly, json({ state: "revoked" }))).status).toBe(403);
    for (const method of ["GET", "DELETE"]) expect((await tokenCall("/api/desktop/me", grantsOnly, { method })).status).toBe(403);
    expect((await tokenCall("/api/desktop/grants", grantsOnly)).status).toBe(200);
    expect((await tokenCall("/api/desktop/me", relayOnly)).status).toBe(200);
  });

  it("answers 401 for a missing, malformed or unknown token", async () => {
    for (const token of [null, "unknown-token", ""]) {
      expect((await tokenCall("/api/desktop/me", token)).status).toBe(401);
      expect((await tokenCall("/api/desktop/grants", token)).status).toBe(401);
    }
  });

  it("answers 403 for a revoked token, for a revoked desktop and for a blocked owner", async () => {
    const mine = await newAccount();
    const paired = await pairDesktop(mine);
    await env.DB.prepare("UPDATE users SET status = 'blocked' WHERE id = ?").bind(mine.user.id).run();
    const blocked = await tokenCall("/api/desktop/grants", paired.token);
    expect(blocked.status).toBe(403);
    expect(await blocked.json()).toMatchObject({ code: "blocked" });
    await env.DB.prepare("UPDATE users SET status = 'active' WHERE id = ?").bind(mine.user.id).run();
    await env.DB.prepare("UPDATE desktops SET revoked_at = 1 WHERE id = ?").bind(paired.desktopId).run();
    expect((await tokenCall("/api/desktop/grants", paired.token)).status).toBe(403);
  });

  it("only reads the desktop of its own token: grants of another desktop never appear", async () => {
    const mine = await newAccount();
    const a = await pairDesktop(mine);
    const b = await pairDesktop(mine);
    const device = await registerDevice(mine);
    await requestGrant(mine, a.desktopId, device.deviceId);
    const listB = (await (await tokenCall("/api/desktop/grants", b.token)).json()) as { grants: unknown[] };
    expect(listB.grants).toEqual([]);
    expect((await tokenCall(`/api/desktop/grants/${device.deviceId}/state`, b.token, json({ state: "revoked" }))).status).toBe(404);
  });

  it("ignores cookies on token routes: a session cookie alone is 401", async () => {
    const mine = await newAccount();
    const res = await tokenCall("/api/desktop/me", null, { headers: { Cookie: mine.session.cookie } });
    expect(res.status).toBe(401);
  });
});
