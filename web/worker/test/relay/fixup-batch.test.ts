// Web fix-up batch (point 8): forgetting a device needs a fresh passkey, logout deletes the push subscriptions of
// the browser's device, the pairing request keeps the desktop's app version, and the outer input bounds.
import { describe, expect, it } from "vitest";
import { loginAs, postJson, freshIp } from "../support/accounts.ts";
import { approve, authedCall, BROWSER_HEADERS, call, env, json, newAccount, pairDesktop, rawStart, randomKey, registerDevice, sha256Base64Url } from "./support/api-support.ts";

const errorCode = async (res: Response) => ((await res.json()) as { code: string }).code;

/** Inserts a subscription row (ids and endpoints are unique per call: the test database is shared by the file). */
async function addSubscription(label: string, userId: string, deviceId: string): Promise<string> {
  const id = `${label}-${crypto.randomUUID()}`;
  await env.DB.prepare("INSERT INTO push_subscriptions (id, user_id, device_id, endpoint, p256dh, auth, vapid_key_id, created_at) VALUES (?, ?, ?, ?, 'p', 'a', 'k', 1)")
    .bind(id, userId, deviceId, `https://push.example/${id}`)
    .run();
  return id;
}

const subscriptionIds = async (...ids: string[]): Promise<string[]> => {
  const rows = (await env.DB.prepare("SELECT id FROM push_subscriptions ORDER BY id").all<{ id: string }>()).results;
  return rows.map((row) => row.id).filter((id) => ids.includes(id));
};
const sessionExists = async (idHash: string): Promise<boolean> => (await env.DB.prepare("SELECT 1 AS one FROM sessions WHERE id_hash = ?").bind(idHash).first()) !== null;

describe("DELETE /api/devices/:id needs a fresh session", () => {
  it("answers 403 fresh_required for a session that did not re-authenticate, and leaves the device alone", async () => {
    const stale = await newAccount({ fresh: false });
    const device = await registerDevice(stale);
    const res = await authedCall(stale.session, `/api/devices/${device.deviceId}`, { method: "DELETE" });
    expect(res.status).toBe(403);
    expect(await errorCode(res)).toBe("fresh_required");
    expect((await env.DB.prepare("SELECT revoked_at FROM devices WHERE id = ?").bind(device.deviceId).first<{ revoked_at: number | null }>())?.revoked_at).toBeNull();
  });

  it("forgets the device for a fresh session", async () => {
    const fresh = await newAccount({ fresh: true });
    const device = await registerDevice(fresh);
    expect((await authedCall(fresh.session, `/api/devices/${device.deviceId}`, { method: "DELETE" })).status).toBe(200);
  });
});

describe("POST /api/auth/logout and push subscriptions", () => {
  it("deletes the push subscriptions of the named device, and only that device of the caller", async () => {
    const mine = await newAccount();
    const other = await newAccount();
    const here = await registerDevice(mine, "Here");
    const elsewhere = await registerDevice(mine, "Elsewhere");
    const foreign = await registerDevice(other, "Foreign");
    const here1 = await addSubscription("here", mine.user.id, here.deviceId);
    const here2 = await addSubscription("here", mine.user.id, here.deviceId);
    const elsewhere1 = await addSubscription("elsewhere", mine.user.id, elsewhere.deviceId);
    const foreign1 = await addSubscription("foreign", other.user.id, foreign.deviceId);

    const res = await authedCall(mine.session, "/api/auth/logout", json({ deviceId: here.deviceId }));
    expect(res.status).toBe(200);
    expect(await sessionExists(mine.session.idHash)).toBe(false);
    expect(await subscriptionIds(here1, here2, elsewhere1, foreign1)).toEqual([elsewhere1, foreign1].sort());
    // The audit row of the logout is still written when the push cleanup rides in the same batch.
    expect(await env.DB.prepare("SELECT count(*) AS n FROM audit_log WHERE actor_user_id = ? AND action = 'session.logout'").bind(mine.user.id).first()).toEqual({ n: 1 });
  });

  it("never deletes the subscriptions of another account's device", async () => {
    const mine = await newAccount();
    const other = await newAccount();
    const foreign = await registerDevice(other, "Foreign");
    const foreign1 = await addSubscription("foreign", other.user.id, foreign.deviceId);
    const res = await authedCall(mine.session, "/api/auth/logout", json({ deviceId: foreign.deviceId }));
    expect(res.status).toBe(200);
    expect(await subscriptionIds(foreign1)).toEqual([foreign1]);
  });

  it("without a body, or with a malformed one, still signs out and keeps the subscriptions", async () => {
    const mine = await newAccount();
    const device = await registerDevice(mine);
    const keep1 = await addSubscription("keep", mine.user.id, device.deviceId);
    expect((await authedCall(mine.session, "/api/auth/logout", { method: "POST" })).status).toBe(200);
    expect(await sessionExists(mine.session.idHash)).toBe(false);

    const again = await newAccount();
    expect((await authedCall(again.session, "/api/auth/logout", { method: "POST", body: "not json" })).status).toBe(200);
    expect(await sessionExists(again.session.idHash)).toBe(false);
    expect(await subscriptionIds(keep1)).toEqual([keep1]);
    const third = await newAccount();
    expect((await authedCall(third.session, "/api/auth/logout", json({ deviceId: 42 }))).status).toBe(200);
    expect(await sessionExists(third.session.idHash)).toBe(false);
    expect(await subscriptionIds(keep1)).toEqual([keep1]);
  });
});

describe("logout of a recovery-only session and of an oversized device id", () => {
  it("a recovery-only session signs out but never deletes push subscriptions", async () => {
    const mine = await newAccount();
    const device = await registerDevice(mine);
    const kept = await addSubscription("kept", mine.user.id, device.deviceId);
    const recovery = await loginAs(mine.user.id, { recoveryOnly: true });
    const res = await authedCall(recovery, "/api/auth/logout", json({ deviceId: device.deviceId }));
    expect(res.status).toBe(200);
    expect(await sessionExists(recovery.idHash)).toBe(false);
    expect(await subscriptionIds(kept)).toEqual([kept]);
  });

  it("a device id over 64 characters is ignored and the session still ends", async () => {
    const mine = await newAccount();
    const res = await authedCall(mine.session, "/api/auth/logout", json({ deviceId: "d".repeat(65) }));
    expect(res.status).toBe(200);
    expect(await sessionExists(mine.session.idHash)).toBe(false);
  });
});

describe("request body size limit", () => {
  it("refuses a body over 32 KiB with 413 before any handler reads it, on public and session routes", async () => {
    const huge = JSON.stringify({ handle: "bob-1", code: "A".repeat(40 * 1024) });
    const pub = await postJson("/api/auth/recover", JSON.parse(huge));
    expect(pub.status).toBe(413);
    expect(await errorCode(pub)).toBe("invalid");
    const mine = await newAccount();
    const session = await authedCall(mine.session, "/api/auth/logout", { method: "POST", body: huge });
    expect(session.status).toBe(413);
    expect(await sessionExists(mine.session.idHash)).toBe(true);
  });

  it("refuses an over-limit body that declares no length (chunked)", async () => {
    const chunk = new TextEncoder().encode("A".repeat(1024));
    const stream = new ReadableStream<Uint8Array>({
      start(controller) {
        controller.enqueue(new TextEncoder().encode('{"handle":"bob-1","code":"'));
        for (let i = 0; i < 40; i += 1) controller.enqueue(chunk);
        controller.enqueue(new TextEncoder().encode('"}'));
        controller.close();
      },
    });
    const res = await call("/api/auth/recover", { method: "POST", headers: { ...BROWSER_HEADERS, "CF-Connecting-IP": freshIp() }, body: stream, duplex: "half" } as RequestInit);
    expect(res.status).toBe(413);
  });

  it("still takes a normal body (a recovery code of 256 characters reaches the schema bound)", async () => {
    const res = await postJson("/api/auth/recover", { handle: "bob-1", code: "A".repeat(256) });
    expect(res.status).toBe(401);
  });
});

describe("pairing keeps the desktop's app version (migration 0005)", () => {
  it("stores it on the request and copies it to the new desktop", async () => {
    const mine = await newAccount();
    const verifier = crypto.getRandomValues(new Uint8Array(32));
    const key = randomKey();
    const res = await rawStart({ publicKey: key.b64, machineName: "Mac", os: "macos", appVersion: "3.4.5", challenge: await sha256Base64Url(verifier) });
    expect(res.status).toBe(201);
    const { userCode, deviceCode } = (await res.json()) as { userCode: string; deviceCode: string };
    const hash = [...new Uint8Array(await crypto.subtle.digest("SHA-256", new TextEncoder().encode(deviceCode)))].map((b) => b.toString(16).padStart(2, "0")).join("");
    expect(await env.DB.prepare("SELECT app_version FROM pairing_requests WHERE device_code_hash = ?").bind(hash).first()).toEqual({ app_version: "3.4.5" });

    const approved = await approve(mine.session, userCode);
    expect(approved.status).toBe(200);
    const { desktopId } = (await approved.json()) as { desktopId: string };
    expect(await env.DB.prepare("SELECT app_version FROM desktops WHERE id = ?").bind(desktopId).first()).toEqual({ app_version: "3.4.5" });
  });

  it("a replace pairing updates the version of the desktop", async () => {
    const mine = await newAccount();
    const first = await pairDesktop(mine);
    await env.DB.prepare("UPDATE desktops SET app_version = '0.0.1' WHERE id = ?").bind(first.desktopId).run();
    await pairDesktop(mine, { replaces: first.desktopId });
    expect(await env.DB.prepare("SELECT app_version FROM desktops WHERE id = ?").bind(first.desktopId).first()).toEqual({ app_version: "1.0.0" });
  });
});

describe("outer input bounds of the schemas", () => {
  it("a recovery code over 256 characters is 400 invalid before any handler runs", async () => {
    const res = await postJson("/api/auth/recover", { handle: "bob-1", code: "A".repeat(257) });
    expect(res.status).toBe(400);
    expect(await errorCode(res)).toBe("invalid");
  });

  it("an invite over 256 characters is 400 invalid", async () => {
    const res = await postJson("/api/auth/register/options", { invite: "i".repeat(257), handle: "bob-1", displayName: "Bob" });
    expect(res.status).toBe(400);
    expect(await errorCode(res)).toBe("invalid");
  });

  it("more than 32 reported transports is 400 invalid", async () => {
    const response = { id: "abc", type: "public-key", response: { transports: Array.from({ length: 33 }, (_, i) => `t${i}`) } };
    const res = await postJson("/api/auth/register/verify", { challengeId: "x", response });
    expect(res.status).toBe(400);
    expect(await errorCode(res)).toBe("invalid");
  });
});
