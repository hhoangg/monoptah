// Web push delivery fix: default prefs, Mono targets without a session id, visible outcomes, push service
// statuses, the Apple Web Push request, GET prefs and POST /api/push/test.
import { env } from "cloudflare:workers";
import { PushPayloadSchema, PushPrefsStateResponse, PushTestResponse } from "@monoptah/protocol";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { fromBase64Url, resetVapidCaches, JWT_LIFETIME_SECONDS } from "../../src/modules/push/vapid.ts";
import { IP_LIMITS } from "../../src/modules/ratelimit/index.ts";
import { resetNotifyState, runNotify, type NotifyDeps } from "../../src/modules/push/notify.ts";
import { topicFor } from "../../src/modules/push/payload.ts";
import { resolvePrefs } from "../../src/modules/push/prefs.ts";
import { sendWebPush } from "../../src/modules/push/webpush.ts";
import { call } from "../helpers.ts";
import { authedCall, createTestDesktop, createTestUser, freshIp, loginAs } from "../support/accounts.ts";
import { createDevice, createWorld, fakeFetch, insertSubscription, newSubscriber, overlayEnv, setPrefs, subscriptionRow, vapidEnv, type SentRequest, type World } from "./support/push-support.ts";

beforeEach(() => {
  resetNotifyState();
  resetVapidCaches();
});
afterEach(() => {
  vi.unstubAllGlobals();
  vi.restoreAllMocks();
});

const NOW = 1_900_000_000_000;
const sid = () => `s-${crypto.randomUUID()}`;
const principal = (w: World) => ({ tokenId: "t", desktopId: w.desktopId, userId: w.userId });
const bodyFor = (w: World, overrides: Record<string, unknown> = {}) => ({ category: "agentFinished", project: "Alpha", status: "finished", sessionId: sid(), deviceIds: [w.deviceId], ...overrides });

async function notifyWith(w: World, body: unknown, fetcher: ReturnType<typeof fakeFetch>, nowMs = NOW) {
  const vapid = await vapidEnv();
  const deps: NotifyDeps = { fetch: fetcher.fetch, nowMs: () => nowMs };
  return runNotify(vapid.env, principal(w), body, deps);
}

async function plain(w: World, request: SentRequest) {
  return JSON.parse(new TextDecoder().decode(await w.subscriber.decrypt(request.body))) as Record<string, unknown>;
}

/** The `push.notify` and `push.test` log lines written while `fn` runs. */
async function logged(fn: () => Promise<unknown>): Promise<Record<string, unknown>[]> {
  const spy = vi.spyOn(console, "log").mockImplementation(() => {});
  try {
    await fn();
    return spy.mock.calls.map((args) => JSON.parse(String(args[0])) as Record<string, unknown>).filter((line) => line["event"] === "push.notify" || line["event"] === "push.test");
  } finally {
    spy.mockRestore();
  }
}

describe("default prefs", () => {
  it("no prefs row sends (the defaults are on); this failed before the fix", async () => {
    const w = await createWorld({ prefs: null });
    const f = fakeFetch();
    expect(await notifyWith(w, bodyFor(w), f)).toEqual({ ok: true, sent: 1 });
    expect(f.sent).toHaveLength(1);
  });

  it("an explicit enabled=0 row or a category set to false still turns it off", async () => {
    const off = await createWorld({ prefs: null });
    await setPrefs(off.userId, off.desktopId, false, { agentFinished: true, agentInput: true, monoMessages: true });
    const f = fakeFetch();
    expect(await notifyWith(off, bodyFor(off), f)).toEqual({ ok: true, sent: 0 });
    const cat = await createWorld({ prefs: { agentFinished: false, agentInput: true, monoMessages: true } });
    expect(await notifyWith(cat, bodyFor(cat), f)).toEqual({ ok: true, sent: 0 });
    expect(f.sent).toHaveLength(0);
  });

  it("a category key missing from a stored row counts as on", async () => {
    const w = await createWorld({ prefs: { agentInput: false } });
    const f = fakeFetch();
    expect(await notifyWith(w, bodyFor(w), f)).toEqual({ ok: true, sent: 1 });
    expect(await notifyWith(w, bodyFor(w, { category: "agentInput", status: "needs_input" }), f)).toEqual({ ok: true, sent: 0 });
  });

  it("resolvePrefs: defaults without a row, tolerant of corrupt JSON", () => {
    expect(resolvePrefs({ enabled: null, categories: null })).toEqual({ enabled: true, stored: false, categories: { agentFinished: true, agentInput: true, monoMessages: true } });
    expect(resolvePrefs({ enabled: 1, categories: "not json" })).toMatchObject({ enabled: true, stored: true, categories: { agentFinished: true } });
    expect(resolvePrefs({ enabled: 0, categories: '{"agentInput":false}' })).toEqual({ enabled: false, stored: true, categories: { agentFinished: true, agentInput: false, monoMessages: true } });
  });
});

describe("Mono target without a session id", () => {
  it("uses the Mono URL, the mono: tag and a Topic from SHA-256 of the tag", async () => {
    const w = await createWorld();
    const f = fakeFetch();
    const monoId = `m-${crypto.randomUUID()}`;
    const body = { category: "monoMessages", project: "Ada", status: "message", monoId, deviceIds: [w.deviceId] };
    expect(await notifyWith(w, body, f)).toEqual({ ok: true, sent: 1 });
    const payload = await plain(w, f.sent[0]!);
    expect(payload).toEqual({ v: 1, t: "Studio Mac", b: "Ada: New message", url: `/d/${w.desktopId}/monos/${monoId}`, tag: `mono:${monoId}` });
    expect(PushPayloadSchema.safeParse(payload).success).toBe(true);
    expect(f.sent[0]!.headers["topic"]).toBe(await topicFor(`mono:${monoId}`));
  });

  it("an empty sessionId next to a monoId is treated as absent", async () => {
    const w = await createWorld();
    const f = fakeFetch();
    const monoId = `m-${crypto.randomUUID()}`;
    expect(await notifyWith(w, { category: "monoMessages", project: "Ada", status: "message", sessionId: "", monoId, deviceIds: [w.deviceId] }, f)).toEqual({ ok: true, sent: 1 });
    expect(await plain(w, f.sent[0]!)).toMatchObject({ url: `/d/${w.desktopId}/monos/${monoId}`, tag: `mono:${monoId}` });
  });

  it("answers 400 with neither id, with an empty sessionId and no monoId, and for a habit run without a session", async () => {
    const w = await createWorld();
    const f = fakeFetch();
    const base = { category: "monoMessages", project: "Ada", status: "message", deviceIds: [w.deviceId] };
    expect(await notifyWith(w, base, f)).toMatchObject({ ok: false, status: 400 });
    expect(await notifyWith(w, { ...base, sessionId: "" }, f)).toMatchObject({ ok: false, status: 400 });
    expect(await notifyWith(w, { ...base, monoId: "m1", habitRun: true }, f)).toMatchObject({ ok: false, status: 400 });
    expect(await notifyWith(w, { ...base, sessionId: "", monoId: "m1", habitRun: true }, f)).toMatchObject({ ok: false, status: 400 });
    expect(await notifyWith(w, { ...base, monoId: "../x" }, f)).toMatchObject({ ok: false, status: 400 });
    expect(f.sent).toHaveLength(0);
  });

  it("dedupes on mono:<monoId> and keeps it apart from a session notice", async () => {
    const w = await createWorld();
    const f = fakeFetch();
    const mono = { category: "monoMessages", project: "Ada", status: "message", monoId: "m-dedupe", deviceIds: [w.deviceId] };
    expect(await notifyWith(w, mono, f)).toEqual({ ok: true, sent: 1 });
    expect(await notifyWith(w, mono, f, NOW + 1000)).toEqual({ ok: true, sent: 0 });
    expect(await notifyWith(w, { ...mono, sessionId: "m-dedupe" }, f, NOW + 1000)).toEqual({ ok: true, sent: 1 });
  });

  it("an old-style body (sessionId, optional monoId) behaves as before", async () => {
    const w = await createWorld();
    const f = fakeFetch();
    const sessionId = sid();
    await notifyWith(w, bodyFor(w, { sessionId }), f);
    expect(await plain(w, f.sent[0]!)).toMatchObject({ url: `/d/${w.desktopId}/s/${sessionId}`, tag: sessionId });
    expect(f.sent[0]!.headers["topic"]).toBe(await topicFor(sessionId));
  });
});

describe("visible outcomes", () => {
  async function outcomeOf(w: World, body: unknown, fetcher = fakeFetch(), vapid?: Awaited<ReturnType<typeof vapidEnv>>) {
    const setup = vapid ?? (await vapidEnv());
    const lines = await logged(() => runNotify(setup.env, principal(w), body, { fetch: fetcher.fetch, nowMs: () => NOW }));
    expect(lines).toHaveLength(1);
    return lines[0]!;
  }

  it("writes exactly one line for every early return, with the outcome and no private data", async () => {
    const w = await createWorld();
    expect((await outcomeOf(w, { nope: 1 }))["outcome"]).toBe("invalid");
    expect((await outcomeOf(w, bodyFor(w, { sessionId: "a b" })))["outcome"]).toBe("invalid");
    expect((await outcomeOf(w, bodyFor(w, { deviceIds: [] })))["outcome"]).toBe("no_devices");
    expect((await outcomeOf(w, bodyFor(w, { deviceIds: ["unknown-device"] })))["outcome"]).toBe("no_recipients");

    const off = await createWorld({ prefs: null });
    await setPrefs(off.userId, off.desktopId, false, { agentFinished: true, agentInput: true, monoMessages: true });
    expect((await outcomeOf(off, bodyFor(off)))["outcome"]).toBe("prefs_off");
    const cat = await createWorld({ prefs: { agentFinished: false, agentInput: true, monoMessages: true } });
    expect((await outcomeOf(cat, bodyFor(cat)))["outcome"]).toBe("category_off");

    const vapid = await outcomeOf(w, bodyFor(w), fakeFetch(), { ...(await vapidEnv()), env: overlayEnv({ VAPID_PUBLIC_KEY: "REPLACE_WITH_VAPID_PUBLIC_KEY_BASE64URL", VAPID_PRIVATE_KEY: "x" }) });
    expect(vapid["outcome"]).toBe("vapid_unusable");
    expect(vapid).toMatchObject({ event: "push.notify", desktopId: w.desktopId, category: "agentFinished", recipients: 1, sent: 0 });
    expect(JSON.stringify(vapid)).not.toContain(w.endpoint);
  });

  it("dedupe is logged as deduped", async () => {
    const w = await createWorld();
    const body = bodyFor(w);
    expect((await outcomeOf(w, body))["outcome"]).toBe("sent");
    expect((await outcomeOf(w, body))["outcome"]).toBe("deduped");
  });

  it("a send logs counts and the push service statuses, never endpoints or keys", async () => {
    const w = await createWorld();
    await insertSubscription(w.userId, w.deviceId, await newSubscriber(), { endpoint: `https://fcm.googleapis.com/fcm/send/${crypto.randomUUID()}/gone` });
    await insertSubscription(w.userId, w.deviceId, await newSubscriber(), { endpoint: `https://fcm.googleapis.com/fcm/send/${crypto.randomUUID()}/limited` });
    await insertSubscription(w.userId, w.deviceId, await newSubscriber(), { endpoint: `https://fcm.googleapis.com/fcm/send/${crypto.randomUUID()}/broken` });
    const f = fakeFetch((url) => new Response(null, { status: url.endsWith("/gone") ? 410 : url.endsWith("/limited") ? 429 : url.endsWith("/broken") ? 403 : 201 }));
    const line = await outcomeOf(w, bodyFor(w), f);
    expect(line).toMatchObject({ outcome: "sent", recipients: 4, sent: 1, gone: 1, failed: 1, backoff: 1, skippedKey: 0 });
    expect([...(line["statuses"] as number[])].sort()).toEqual([201, 403, 410, 429]);
    expect(JSON.stringify(line)).not.toMatch(/fcm\.googleapis|p256dh|auth/);
  });

  it("writes one push.notify audit row when something went wrong, with metadata only", async () => {
    const w = await createWorld();
    await outcomeOf(w, bodyFor(w), fakeFetch(410));
    const row = await env.DB.prepare("SELECT actor_kind, actor_user_id, target_id, meta FROM audit_log WHERE action = 'push.notify' AND target_id = ?").bind(w.desktopId).first<{ actor_kind: string; actor_user_id: string; target_id: string; meta: string }>();
    expect(row).toMatchObject({ actor_kind: "desktop", actor_user_id: w.userId });
    expect(JSON.parse(row!.meta)).toEqual({ desktopId: w.desktopId, category: "agentFinished", recipients: 1, sent: 0, gone: 1, failed: 0, backoff: 0 });
    // A clean success and early returns write no audit row.
    const ok = await createWorld();
    await outcomeOf(ok, bodyFor(ok), fakeFetch());
    expect(await env.DB.prepare("SELECT 1 FROM audit_log WHERE action = 'push.notify' AND target_id = ?").bind(ok.desktopId).first()).toBeNull();
    const off = await createWorld({ prefs: { agentFinished: false, agentInput: true, monoMessages: true } });
    await outcomeOf(off, bodyFor(off));
    expect(await env.DB.prepare("SELECT 1 FROM audit_log WHERE action = 'push.notify' AND target_id = ?").bind(off.desktopId).first()).toBeNull();
  });

  it("still answers {sent} and logs write_failed when the post-send batch throws", async () => {
    const w = await createWorld();
    const vapid = await vapidEnv();
    const failingDb = { prepare: (sql: string) => env.DB.prepare(sql), batch: async () => { throw new Error("d1 down"); } } as unknown as D1Database;
    const lines = await logged(async () => {
      const result = await runNotify(overlayEnv({ ...Object.fromEntries(Object.entries(vapid.env)), DB: failingDb }), principal(w), bodyFor(w), { fetch: fakeFetch(410).fetch, nowMs: () => NOW });
      expect(result).toEqual({ ok: true, sent: 0 });
    });
    expect(lines).toHaveLength(1);
    expect(lines[0]).toMatchObject({ event: "push.notify", outcome: "write_failed", recipients: 1, gone: 1 });
  });
});

describe("status of the push service answer", () => {
  const target = { endpoint: "https://web.push.apple.com/x", p256dh: "", auth: "" };
  it("carries the HTTP status for ok, gone, backoff and failed, and 0 for a network failure", async () => {
    const subscriber = await newSubscriber();
    const t = { ...target, p256dh: subscriber.p256dh, auth: subscriber.auth };
    const headers = { authorization: "vapid t=a.b.c, k=k", urgency: "normal" as const, topic: "t" };
    const send = (answer: () => Promise<Response>) => sendWebPush(answer, t, new Uint8Array([1]), headers, NOW);
    expect(await send(async () => new Response(null, { status: 201 }))).toEqual({ kind: "ok", status: 201 });
    expect(await send(async () => new Response(null, { status: 410 }))).toEqual({ kind: "gone", status: 410 });
    expect(await send(async () => new Response(null, { status: 404 }))).toEqual({ kind: "gone", status: 404 });
    expect(await send(async () => new Response(null, { status: 429, headers: { "Retry-After": "7" } }))).toEqual({ kind: "backoff", status: 429, retryAfterSeconds: 7 });
    expect(await send(async () => new Response(null, { status: 403 }))).toEqual({ kind: "failed", status: 403 });
    expect(await send(async () => new Response(null, { status: 500 }))).toEqual({ kind: "failed", status: 500 });
    expect(await send(() => Promise.reject(new Error("offline")))).toEqual({ kind: "failed", status: 0 });
  });
});

describe("Apple Web Push request", () => {
  it("signs for the exact origin, with sub, exp within 24 h and exactly the documented headers", async () => {
    const w = await createWorld();
    const endpoint = `https://web.push.apple.com/${crypto.randomUUID()}`;
    await env.DB.prepare("DELETE FROM push_subscriptions WHERE id = ?").bind(w.subscriptionId).run();
    await insertSubscription(w.userId, w.deviceId, w.subscriber, { endpoint });
    const f = fakeFetch();
    const sessionId = sid();
    expect(await notifyWith(w, bodyFor(w, { sessionId, category: "agentInput", status: "needs_input" }), f)).toEqual({ ok: true, sent: 1 });
    const request = f.sent[0]!;
    expect(request.url).toBe(endpoint);
    const match = /^vapid t=([\w-]+)\.([\w-]+)\.([\w-]+), k=([\w-]+)$/.exec(request.headers["authorization"] ?? "") as RegExpExecArray;
    expect(match).not.toBeNull();
    const claims = JSON.parse(new TextDecoder().decode(fromBase64Url(match[2]!))) as { aud: string; sub: string; exp: number };
    expect(claims.aud).toBe("https://web.push.apple.com");
    expect(claims.sub).toBe("mailto:test@example.invalid");
    const nowSec = Math.floor(NOW / 1000);
    expect(claims.exp - nowSec).toBeGreaterThan(0);
    expect(claims.exp - nowSec).toBeLessThanOrEqual(86_400);
    expect(JWT_LIFETIME_SECONDS).toBeLessThanOrEqual(86_400);
    expect(Object.keys(claims).sort()).toEqual(["aud", "exp", "sub"]);
    expect(Object.keys(request.headers).sort()).toEqual(["authorization", "content-encoding", "content-type", "topic", "ttl", "urgency"]);
    expect(request.headers).toMatchObject({ ttl: "86400", urgency: "high", "content-encoding": "aes128gcm", "content-type": "application/octet-stream" });
    expect(request.headers["topic"]).toMatch(/^[A-Za-z0-9_-]{1,32}$/);
  });
});

describe("GET /api/push/prefs/:desktopId", () => {
  async function signedIn() {
    const user = await createTestUser(env);
    return { user, session: await loginAs(user.id) };
  }
  const get = (session: { cookie: string }, desktopId: string) => authedCall(session, `/api/push/prefs/${desktopId}`);
  const all = { agentFinished: true, agentInput: true, monoMessages: true };

  it("returns the defaults with stored false when no row exists, then the stored row", async () => {
    const { user, session } = await signedIn();
    const desktopId = await createTestDesktop(env, user.id);
    const res = await get(session, desktopId);
    expect(res.status).toBe(200);
    const body = PushPrefsStateResponse.parse(await res.json());
    expect(body).toEqual({ enabled: true, categories: all, stored: false });
    await setPrefs(user.id, desktopId, false, { agentFinished: false, agentInput: true, monoMessages: true });
    expect(await (await get(session, desktopId)).json()).toEqual({ enabled: false, categories: { ...all, agentFinished: false }, stored: true });
  });

  it("fills a category missing from the stored JSON as on", async () => {
    const { user, session } = await signedIn();
    const desktopId = await createTestDesktop(env, user.id);
    await setPrefs(user.id, desktopId, true, { agentInput: false });
    expect(await (await get(session, desktopId)).json()).toEqual({ enabled: true, categories: { ...all, agentInput: false }, stored: true });
  });

  it("answers 404 for a foreign, unknown or revoked desktop and 401 without a session", async () => {
    const a = await signedIn();
    const b = await signedIn();
    expect((await get(a.session, await createTestDesktop(env, b.user.id))).status).toBe(404);
    expect((await get(a.session, crypto.randomUUID())).status).toBe(404);
    expect((await get(a.session, await createTestDesktop(env, a.user.id, { revoked: true }))).status).toBe(404);
    const anonymous = await call("/api/push/prefs/x");
    expect(anonymous.status).toBe(401);
  });
});

describe("POST /api/push/test", () => {
  async function setup() {
    const user = await createTestUser(env);
    const session = await loginAs(user.id);
    const vapid = await vapidEnv();
    const deviceId = await createDevice(user.id);
    return { user, session, vapid, deviceId, ip: freshIp() };
  }
  const post = (s: Awaited<ReturnType<typeof setup>>, deviceId: string) => authedCall(s.session, "/api/push/test", { method: "POST", headers: { "CF-Connecting-IP": s.ip }, body: JSON.stringify({ deviceId }) }, s.vapid.env);

  it("sends the fixed test payload to every subscription of the own device and reports statuses", async () => {
    const s = await setup();
    const subscriber = await newSubscriber();
    const okEndpoint = `https://web.push.apple.com/${crypto.randomUUID()}`;
    const ok = await insertSubscription(s.user.id, s.deviceId, subscriber, { endpoint: okEndpoint });
    await insertSubscription(s.user.id, s.deviceId, await newSubscriber(), { endpoint: `https://fcm.googleapis.com/fcm/send/${crypto.randomUUID()}/boom` });
    await insertSubscription(s.user.id, s.deviceId, await newSubscriber(), { endpoint: `https://fcm.googleapis.com/fcm/send/${crypto.randomUUID()}/offline` });
    await insertSubscription(s.user.id, s.deviceId, await newSubscriber(), { endpoint: `https://fcm.googleapis.com/fcm/send/${crypto.randomUUID()}/retired`, vapidKeyId: "retired" });
    const requests: { url: string; headers: Headers; body: Uint8Array }[] = [];
    vi.stubGlobal("fetch", async (url: string, init: RequestInit) => {
      requests.push({ url, headers: new Headers(init.headers), body: new Uint8Array(init.body as ArrayBuffer) });
      if (url.endsWith("/offline")) throw new Error("offline");
      return new Response(null, { status: url.endsWith("/boom") ? 403 : 201 });
    });
    const res = await post(s, s.deviceId);
    expect(res.status).toBe(200);
    const body = PushTestResponse.parse(await res.json());
    expect(body.sent).toBe(1);
    expect([...body.results].sort((a, b) => a.outcome.localeCompare(b.outcome) || a.status - b.status)).toEqual([
      { outcome: "failed", status: 0 },
      { outcome: "failed", status: 403 },
      { outcome: "ok", status: 201 },
      { outcome: "skipped_key", status: 0 },
    ]);
    const sentToOk = requests.find((r) => r.url === okEndpoint)!;
    expect(JSON.parse(new TextDecoder().decode(await subscriber.decrypt(sentToOk.body)))).toEqual({ v: 1, t: "Monoptah", b: "Test notification", url: "/settings", tag: "monoptah-test" });
    expect(sentToOk.headers.get("urgency")).toBe("high");
    expect(sentToOk.headers.get("topic")).toBe(await topicFor("monoptah-test"));
    expect((await subscriptionRow(ok))?.last_success_at).not.toBeNull();
    const audit = await env.DB.prepare("SELECT actor_kind, actor_user_id, meta FROM audit_log WHERE action = 'push.test' AND actor_user_id = ?").bind(s.user.id).first<{ actor_kind: string; meta: string }>();
    expect(audit?.actor_kind).toBe("user");
    expect(JSON.parse(audit!.meta)).toEqual({ deviceId: s.deviceId, recipients: 4, sent: 1, gone: 0, failed: 2, backoff: 0 });
  });

  it("deletes a subscription the push service reports gone (410) and counts a failure otherwise", async () => {
    const s = await setup();
    const gone = await insertSubscription(s.user.id, s.deviceId, await newSubscriber(), { endpoint: `https://fcm.googleapis.com/fcm/send/${crypto.randomUUID()}/gone` });
    const broken = await insertSubscription(s.user.id, s.deviceId, await newSubscriber(), { endpoint: `https://fcm.googleapis.com/fcm/send/${crypto.randomUUID()}/broken` });
    vi.stubGlobal("fetch", async (url: string) => new Response(null, { status: url.endsWith("/gone") ? 410 : 500 }));
    const body = PushTestResponse.parse(await (await post(s, s.deviceId)).json());
    expect(body.sent).toBe(0);
    expect(await subscriptionRow(gone)).toBeNull();
    expect((await subscriptionRow(broken))?.failures).toBe(1);
  });

  it("answers {sent:0, results:[]} for a device with no subscription", async () => {
    const s = await setup();
    vi.stubGlobal("fetch", async () => new Response(null, { status: 201 }));
    const res = await post(s, s.deviceId);
    expect(res.status).toBe(200);
    expect(await res.json()).toEqual({ sent: 0, results: [] });
  });

  it("answers 404 for another user's device, an unknown device and a revoked one", async () => {
    const s = await setup();
    const stranger = await createTestUser(env);
    expect((await post(s, await createDevice(stranger.id))).status).toBe(404);
    expect((await post(s, crypto.randomUUID())).status).toBe(404);
    expect((await post(s, await createDevice(s.user.id, { revoked: true }))).status).toBe(404);
  });

  it("answers 400 for a bad body and 503 when the VAPID key is unusable", async () => {
    const s = await setup();
    expect((await authedCall(s.session, "/api/push/test", { method: "POST", body: JSON.stringify({}) }, s.vapid.env)).status).toBe(400);
    await insertSubscription(s.user.id, s.deviceId, await newSubscriber());
    const bad = overlayEnv({ VAPID_PUBLIC_KEY: "REPLACE_WITH_VAPID_PUBLIC_KEY_BASE64URL", VAPID_PRIVATE_KEY: "x" });
    const res = await authedCall(s.session, "/api/push/test", { method: "POST", body: JSON.stringify({ deviceId: s.deviceId }) }, bad);
    expect(res.status).toBe(503);
  });

  it("allows 3 calls per user per 10 minutes and answers 429 with Retry-After on the 4th", async () => {
    const s = await setup();
    vi.stubGlobal("fetch", async () => new Response(null, { status: 201 }));
    for (let i = 0; i < 3; i++) expect((await post(s, s.deviceId)).status, `call ${i + 1}`).toBe(200);
    const limited = await post(s, s.deviceId);
    expect(limited.status).toBe(429);
    expect(Number(limited.headers.get("Retry-After"))).toBeGreaterThan(0);
    // Another user is not affected.
    const other = await setup();
    expect((await post(other, other.deviceId)).status).toBe(200);
  });

  it("still answers the result and logs write_failed when the post-send batch throws", async () => {
    const s = await setup();
    await insertSubscription(s.user.id, s.deviceId, await newSubscriber());
    vi.stubGlobal("fetch", async () => new Response(null, { status: 201 }));
    const failingDb = { prepare: (sql: string) => env.DB.prepare(sql), batch: async () => { throw new Error("d1 down"); } } as unknown as D1Database;
    let res: Response | undefined;
    const lines = await logged(async () => {
      res = await authedCall(s.session, "/api/push/test", { method: "POST", headers: { "CF-Connecting-IP": s.ip }, body: JSON.stringify({ deviceId: s.deviceId }) }, overlayEnv({ ...Object.fromEntries(Object.entries(s.vapid.env)), DB: failingDb }));
    });
    expect(res!.status).toBe(200);
    expect(PushTestResponse.parse(await res!.json()).sent).toBe(1);
    expect(lines).toHaveLength(1);
    expect(lines[0]).toMatchObject({ event: "push.test", outcome: "write_failed", sent: 1 });
  });

  it("keys the limit by a hash of the user id and audits the breach with the user as actor", async () => {
    const s = await setup();
    vi.stubGlobal("fetch", async () => new Response(null, { status: 201 }));
    for (let i = 0; i < 4; i++) await post(s, s.deviceId);
    const raw = await env.DB.prepare("SELECT 1 FROM rate_limits WHERE instr(key, ?) > 0").bind(s.user.id).first();
    expect(raw).toBeNull();
    const breach = await env.DB.prepare("SELECT actor_kind FROM audit_log WHERE action = 'ratelimit.breach' AND actor_user_id = ?").bind(s.user.id).first<{ actor_kind: string }>();
    expect(breach?.actor_kind).toBe("user");
  });

  it("has no per-IP limit, so an unauthenticated call is a 401 with no rate_limits write", async () => {
    expect(IP_LIMITS["push-test"]).toBeNull();
    const ip = freshIp();
    const before = (await env.DB.prepare("SELECT COUNT(*) AS n FROM rate_limits").first<{ n: number }>())!.n;
    const res = await call("/api/push/test", { method: "POST", headers: { "CF-Connecting-IP": ip, Origin: env.RP_ORIGIN, "X-Monoptah-CSRF": "1", "Content-Type": "application/json" }, body: "{}" });
    expect(res.status).toBe(401);
    expect((await env.DB.prepare("SELECT COUNT(*) AS n FROM rate_limits").first<{ n: number }>())!.n).toBe(before);
  });

  it("requires a session and logs one push.test line without private data", async () => {
    expect((await call("/api/push/test", { method: "POST", headers: { Origin: env.RP_ORIGIN, "X-Monoptah-CSRF": "1", "Content-Type": "application/json" }, body: "{}" })).status).toBe(401);
    const s = await setup();
    await insertSubscription(s.user.id, s.deviceId, await newSubscriber(), { endpoint: `https://web.push.apple.com/secret-endpoint-${crypto.randomUUID()}` });
    vi.stubGlobal("fetch", async () => new Response(null, { status: 201 }));
    const lines = await logged(() => post(s, s.deviceId));
    expect(lines).toHaveLength(1);
    expect(lines[0]).toMatchObject({ event: "push.test", outcome: "sent", recipients: 1, sent: 1, statuses: [201] });
    expect(JSON.stringify(lines[0])).not.toContain("secret-endpoint");
  });
});
