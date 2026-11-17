// P-01: VAPID key, subscribe and unsubscribe, endpoint and key validation, per device cap, upsert, prefs.
import { env } from "cloudflare:workers";
import { describe, expect, it } from "vitest";
import { isAllowedEndpoint, MAX_SUBSCRIPTIONS_PER_DEVICE } from "../../src/modules/push/subscriptions.ts";
import { toBase64Url } from "../../src/lib/crypto.ts";
import { authedCall, createTestDesktop, createTestUser, loginAs } from "../support/accounts.ts";
import { BROWSER_HEADERS, call } from "../helpers.ts";
import { createDevice, newSubscriber, subscriptionRow } from "./support/push-support.ts";

const KEY_ID = env.VAPID_KEY_ID;

async function signedInUser() {
  const user = await createTestUser(env);
  const session = await loginAs(user.id);
  return { user, session };
}

async function subscribeBody(deviceId: string, overrides: Record<string, unknown> = {}) {
  const subscriber = await newSubscriber();
  return {
    endpoint: `https://fcm.googleapis.com/fcm/send/${crypto.randomUUID()}`,
    keys: { p256dh: subscriber.p256dh, auth: subscriber.auth },
    deviceId,
    vapidKeyId: KEY_ID,
    ...overrides,
  };
}

function post(session: { cookie: string }, body: unknown) {
  return authedCall(session, "/api/push/subscriptions", { method: "POST", body: JSON.stringify(body) });
}

describe("GET /api/push/vapid-key", () => {
  it("is public and returns the configured public key and key id", async () => {
    const res = await call("/api/push/vapid-key");
    expect(res.status).toBe(200);
    expect(await res.json()).toEqual({ publicKey: env.VAPID_PUBLIC_KEY, keyId: env.VAPID_KEY_ID });
  });
});

describe("subscribe and unsubscribe", () => {
  it("stores a subscription and removes it again", async () => {
    const { user, session } = await signedInUser();
    const deviceId = await createDevice(user.id);
    const body = await subscribeBody(deviceId);
    const res = await post(session, body);
    expect(res.status).toBe(200);
    const { id } = (await res.json()) as { id: string };
    const row = await subscriptionRow(id);
    expect(row).toMatchObject({ user_id: user.id, device_id: deviceId, endpoint: body.endpoint, vapid_key_id: KEY_ID, failures: 0 });

    const del = await authedCall(session, `/api/push/subscriptions/${id}`, { method: "DELETE" });
    expect(del.status).toBe(200);
    expect(await subscriptionRow(id)).toBeNull();
    expect((await authedCall(session, `/api/push/subscriptions/${id}`, { method: "DELETE" })).status).toBe(404);
  });

  it("requires a session", async () => {
    expect((await call("/api/push/subscriptions", { method: "POST", headers: BROWSER_HEADERS, body: "{}" })).status).toBe(401);
  });

  it("refuses to delete another user's subscription", async () => {
    const a = await signedInUser();
    const b = await signedInUser();
    const res = await post(a.session, await subscribeBody(await createDevice(a.user.id)));
    const { id } = (await res.json()) as { id: string };
    expect((await authedCall(b.session, `/api/push/subscriptions/${id}`, { method: "DELETE" })).status).toBe(404);
    expect(await subscriptionRow(id)).not.toBeNull();
  });
});

describe("endpoint allowlist and key validation", () => {
  it("accepts the known push service hosts", () => {
    for (const host of ["fcm.googleapis.com", "updates.push.services.mozilla.com", "web.push.apple.com", "x.push.apple.com", "wns2-par02p.notify.windows.com"]) {
      expect(isAllowedEndpoint(`https://${host}/send/abc`), host).toBe(true);
    }
  });

  it("refuses other hosts, plain http, look-alikes, credentials and long endpoints", () => {
    for (const endpoint of [
      "https://example.com/push",
      "http://fcm.googleapis.com/fcm/send/x",
      "https://fcm.googleapis.com.evil.example/x",
      "https://evilpush.apple.com.example/x",
      "https://notpush.apple.com/x",
      "https://user:pass@fcm.googleapis.com/x",
      "https://fcm.googleapis.com:8443/x",
      "https://127.0.0.1/x",
      `https://fcm.googleapis.com/${"a".repeat(2048)}`,
      "not a url",
    ]) {
      expect(isAllowedEndpoint(endpoint), endpoint).toBe(false);
    }
  });

  it("answers 400 for a disallowed endpoint, bad key lengths and a stale VAPID key id", async () => {
    const { user, session } = await signedInUser();
    const deviceId = await createDevice(user.id);
    const bad = async (overrides: Record<string, unknown>) => (await post(session, await subscribeBody(deviceId, overrides))).status;
    expect(await bad({ endpoint: "https://example.com/push" })).toBe(400);
    expect(await bad({ endpoint: "http://fcm.googleapis.com/fcm/send/x" })).toBe(400);
    const good = await newSubscriber();
    expect(await bad({ keys: { p256dh: toBase64Url(new Uint8Array(64)), auth: good.auth } })).toBe(400);
    expect(await bad({ keys: { p256dh: toBase64Url(new Uint8Array([1, ...new Uint8Array(64)])), auth: good.auth } })).toBe(400);
    expect(await bad({ keys: { p256dh: good.p256dh, auth: toBase64Url(new Uint8Array(15)) } })).toBe(400);
    expect(await bad({ keys: { p256dh: good.p256dh, auth: toBase64Url(new Uint8Array(17)) } })).toBe(400);
    expect(await bad({ keys: { p256dh: "***", auth: good.auth } })).toBe(400);
    expect(await bad({ vapidKeyId: "other-key" })).toBe(400);
    expect((await post(session, { endpoint: 1 })).status).toBe(400);
  });
});

describe("device ownership", () => {
  it("refuses another user's device, an unknown device and a revoked device with 404", async () => {
    const a = await signedInUser();
    const b = await signedInUser();
    const foreign = await createDevice(b.user.id);
    expect((await post(a.session, await subscribeBody(foreign))).status).toBe(404);
    expect((await post(a.session, await subscribeBody(crypto.randomUUID()))).status).toBe(404);
    const revoked = await createDevice(a.user.id, { revoked: true });
    expect((await post(a.session, await subscribeBody(revoked))).status).toBe(404);
  });
});

describe("per device cap", () => {
  it("allows 4 subscriptions per device and answers 409 quota_exceeded for the fifth, while a re-subscribe still works", async () => {
    const { user, session } = await signedInUser();
    const deviceId = await createDevice(user.id);
    const bodies = [];
    for (let i = 0; i < MAX_SUBSCRIPTIONS_PER_DEVICE; i++) {
      const body = await subscribeBody(deviceId);
      bodies.push(body);
      expect((await post(session, body)).status).toBe(200);
    }
    const fifth = await post(session, await subscribeBody(deviceId));
    expect(fifth.status).toBe(409);
    expect(await fifth.json()).toMatchObject({ code: "quota_exceeded" });
    expect((await post(session, bodies[0])).status).toBe(200);
    const other = await createDevice(user.id);
    expect((await post(session, await subscribeBody(other))).status).toBe(200);
  });
});

describe("endpoint is unique", () => {
  it("upserts: the same endpoint keeps one row, takes the new keys and resets failures", async () => {
    const { user, session } = await signedInUser();
    const deviceId = await createDevice(user.id);
    const body = await subscribeBody(deviceId);
    const first = (await (await post(session, body)).json()) as { id: string };
    await env.DB.prepare("UPDATE push_subscriptions SET failures = 3 WHERE id = ?").bind(first.id).run();
    const fresh = await newSubscriber();
    const second = (await (await post(session, { ...body, keys: { p256dh: fresh.p256dh, auth: fresh.auth } })).json()) as { id: string };
    expect(second.id).toBe(first.id);
    const count = await env.DB.prepare("SELECT count(*) AS n FROM push_subscriptions WHERE endpoint = ?").bind(body.endpoint).first<{ n: number }>();
    expect(count?.n).toBe(1);
    const row = await env.DB.prepare("SELECT p256dh, auth, failures FROM push_subscriptions WHERE id = ?").bind(first.id).first();
    expect(row).toEqual({ p256dh: fresh.p256dh, auth: fresh.auth, failures: 0 });
  });

  it("moves an endpoint to the user and device that registers it last", async () => {
    const a = await signedInUser();
    const b = await signedInUser();
    const deviceA = await createDevice(a.user.id);
    const deviceB = await createDevice(b.user.id);
    const body = await subscribeBody(deviceA);
    const { id } = (await (await post(a.session, body)).json()) as { id: string };
    expect((await post(b.session, { ...body, deviceId: deviceB })).status).toBe(200);
    expect(await subscriptionRow(id)).toMatchObject({ user_id: b.user.id, device_id: deviceB });
  });
});

describe("PUT /api/push/prefs/:desktopId", () => {
  const all = { agentFinished: true, agentInput: false, monoMessages: true };
  const put = (session: { cookie: string }, desktopId: string, body: unknown) =>
    authedCall(session, `/api/push/prefs/${desktopId}`, { method: "PUT", body: JSON.stringify(body) });

  it("stores enabled and the three categories and returns them; a second PUT replaces them", async () => {
    const { user, session } = await signedInUser();
    const desktopId = await createTestDesktop(env, user.id);
    const res = await put(session, desktopId, { enabled: true, categories: all });
    expect(res.status).toBe(200);
    expect(await res.json()).toEqual({ enabled: true, categories: all });
    const row = await env.DB.prepare("SELECT enabled, categories FROM push_prefs WHERE user_id = ? AND desktop_id = ?").bind(user.id, desktopId).first<{ enabled: number; categories: string }>();
    expect(row?.enabled).toBe(1);
    expect(JSON.parse(row?.categories ?? "{}")).toEqual(all);
    const off = await put(session, desktopId, { enabled: false, categories: { agentFinished: false, agentInput: false, monoMessages: false } });
    expect(await off.json()).toEqual({ enabled: false, categories: { agentFinished: false, agentInput: false, monoMessages: false } });
    const count = await env.DB.prepare("SELECT count(*) AS n FROM push_prefs WHERE desktop_id = ?").bind(desktopId).first<{ n: number }>();
    expect(count?.n).toBe(1);
  });

  it("rejects unknown or missing category keys, so only the three known keys are ever stored", async () => {
    const { user, session } = await signedInUser();
    const desktopId = await createTestDesktop(env, user.id);
    expect((await put(session, desktopId, { enabled: true, categories: { ...all, other: true } })).status).toBe(400);
    expect((await put(session, desktopId, { enabled: true, categories: { agentFinished: true } })).status).toBe(400);
    expect(await env.DB.prepare("SELECT count(*) AS n FROM push_prefs WHERE desktop_id = ?").bind(desktopId).first()).toEqual({ n: 0 });
  });

  it("answers 404 for a desktop the caller does not own, an unknown one and a revoked one", async () => {
    const a = await signedInUser();
    const b = await signedInUser();
    const foreign = await createTestDesktop(env, b.user.id);
    expect((await put(a.session, foreign, { enabled: true, categories: all })).status).toBe(404);
    expect((await put(a.session, crypto.randomUUID(), { enabled: true, categories: all })).status).toBe(404);
    const revoked = await createTestDesktop(env, a.user.id, { revoked: true });
    expect((await put(a.session, revoked, { enabled: true, categories: all })).status).toBe(404);
  });

  it("answers 400 for an invalid body", async () => {
    const { user, session } = await signedInUser();
    const desktopId = await createTestDesktop(env, user.id);
    expect((await put(session, desktopId, { enabled: "yes", categories: all })).status).toBe(400);
  });
});
