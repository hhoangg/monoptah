// P-02: POST /api/push/notify: prefs, recipients, payload, headers, push service answers, dedupe, token scope, dual key.
import { env } from "cloudflare:workers";
import { PushPayloadSchema } from "@monoptah/protocol";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { MAX_RECIPIENTS, resetNotifyState, runNotify, type NotifyDeps } from "../../src/modules/push/notify.ts";
import { topicFor } from "../../src/modules/push/payload.ts";
import { fromBase64Url, resetVapidCaches } from "../../src/modules/push/vapid.ts";
import { callWithEnv } from "../helpers.ts";
import { createTestDesktop, createTestUser } from "../support/accounts.ts";
import {
  createDevice,
  createToken,
  createWorld,
  fakeFetch,
  insertSubscription,
  newSubscriber,
  overlayEnv,
  setGrant,
  setPrefs,
  subscriptionRow,
  vapidEnv,
  type SentRequest,
  type World,
} from "./support/push-support.ts";

beforeEach(() => {
  resetNotifyState();
  resetVapidCaches();
});
afterEach(() => vi.unstubAllGlobals());

const NOW = 1_900_000_000_000;
const sid = () => `s-${crypto.randomUUID()}`;

function principal(w: World) {
  return { tokenId: "t", desktopId: w.desktopId, userId: w.userId };
}

function bodyFor(w: World, overrides: Record<string, unknown> = {}) {
  return { category: "agentFinished", project: "Alpha", status: "finished", sessionId: sid(), deviceIds: [w.deviceId], ...overrides };
}

async function notifyWith(w: World, body: unknown, fetcher: ReturnType<typeof fakeFetch>, options: { vapid?: Awaited<ReturnType<typeof vapidEnv>>; nowMs?: number } = {}) {
  const vapid = options.vapid ?? (await vapidEnv());
  const deps: NotifyDeps = { fetch: fetcher.fetch, nowMs: () => options.nowMs ?? NOW };
  return runNotify(vapid.env, principal(w), body, deps);
}

async function plain(w: World, request: SentRequest) {
  return JSON.parse(new TextDecoder().decode(await w.subscriber.decrypt(request.body))) as Record<string, unknown>;
}

describe("preferences", () => {
  it("sends nothing when the category is off or push is disabled; no prefs row means the defaults (on)", async () => {
    const off = await createWorld({ prefs: { agentFinished: false, agentInput: true, monoMessages: true } });
    const f1 = fakeFetch();
    expect(await notifyWith(off, bodyFor(off), f1)).toEqual({ ok: true, sent: 0 });
    expect(f1.sent).toHaveLength(0);
    expect(await notifyWith(off, bodyFor(off, { category: "agentInput", status: "needs_input" }), f1)).toEqual({ ok: true, sent: 1 });

    const disabled = await createWorld();
    await setPrefs(disabled.userId, disabled.desktopId, false, { agentFinished: true, agentInput: true, monoMessages: true });
    const f2 = fakeFetch();
    expect(await notifyWith(disabled, bodyFor(disabled), f2)).toEqual({ ok: true, sent: 0 });
    expect(f2.sent).toHaveLength(0);

    const none = await createWorld({ prefs: null });
    const f3 = fakeFetch();
    expect(await notifyWith(none, bodyFor(none), f3)).toEqual({ ok: true, sent: 1 });
    expect(f3.sent).toHaveLength(1);
  });
});

describe("payload", () => {
  it("round trip: the subscriber decrypts exactly {v,t,b,url,tag} and nothing from extra request fields", async () => {
    const w = await createWorld();
    const f = fakeFetch();
    const sessionId = sid();
    const result = await notifyWith(w, bodyFor(w, { sessionId, title: "SECRET TITLE", text: "SECRET ASSISTANT TEXT", message: "SECRET MESSAGE", transcript: "SECRET TRANSCRIPT" }), f);
    expect(result).toEqual({ ok: true, sent: 1 });
    const text = new TextDecoder().decode(await w.subscriber.decrypt(f.sent[0]!.body));
    expect(text).not.toMatch(/SECRET/);
    const payload = JSON.parse(text) as Record<string, unknown>;
    expect(Object.keys(payload)).toEqual(["v", "t", "b", "url", "tag"]);
    expect(payload).toEqual({ v: 1, t: "Studio Mac", b: "Alpha: Finished", url: `/d/${w.desktopId}/s/${sessionId}`, tag: sessionId });
    expect(PushPayloadSchema.safeParse(payload).success).toBe(true);
    expect(JSON.stringify(f.sent[0]!.headers)).not.toMatch(/SECRET/);
  });

  it("maps the three statuses and prefers desktopName from the request", async () => {
    const w = await createWorld();
    const f = fakeFetch();
    await notifyWith(w, bodyFor(w, { status: "needs_input", category: "agentInput", desktopName: "Renamed" }), f);
    await notifyWith(w, bodyFor(w, { status: "message", category: "monoMessages" }), f);
    expect(await plain(w, f.sent[0]!)).toMatchObject({ t: "Renamed", b: "Alpha: Needs your input" });
    expect(await plain(w, f.sent[1]!)).toMatchObject({ t: "Studio Mac", b: "Alpha: New message" });
  });

  it("URL rule: a Mono message opens the chat, a habit run opens its session, no monoId opens the session", async () => {
    const w = await createWorld();
    const f = fakeFetch();
    const withMono = sid();
    const habit = sid();
    const plainSession = sid();
    await notifyWith(w, bodyFor(w, { sessionId: withMono, monoId: "mono-1", category: "monoMessages", status: "message" }), f);
    await notifyWith(w, bodyFor(w, { sessionId: habit, monoId: "mono-1", habitRun: true, category: "monoMessages", status: "message" }), f);
    await notifyWith(w, bodyFor(w, { sessionId: plainSession }), f);
    expect((await plain(w, f.sent[0]!))["url"]).toBe(`/d/${w.desktopId}/monos/mono-1`);
    expect((await plain(w, f.sent[1]!))["url"]).toBe(`/d/${w.desktopId}/s/${habit}`);
    expect((await plain(w, f.sent[2]!))["url"]).toBe(`/d/${w.desktopId}/s/${plainSession}`);
  });

  it("encodes each URL segment", async () => {
    const w = await createWorld();
    const f = fakeFetch();
    const sessionId = `a:b.c_d-${crypto.randomUUID().slice(0, 8)}`;
    await notifyWith(w, bodyFor(w, { sessionId }), f);
    expect((await plain(w, f.sent[0]!))["url"]).toBe(`/d/${w.desktopId}/s/${encodeURIComponent(sessionId)}`);
  });

  it("stays under 1 KiB with maximal inputs", async () => {
    const w = await createWorld();
    await env.DB.prepare("UPDATE desktops SET name = ? WHERE id = ?").bind("\u{1F600}".repeat(500), w.desktopId).run();
    const f = fakeFetch();
    const big = `${":".repeat(100)}${"x".repeat(28)}`;
    await notifyWith(w, bodyFor(w, { project: "\u{1F600}".repeat(500), sessionId: big, monoId: big, habitRun: true, status: "needs_input", category: "agentInput" }), f);
    const plaintext = await w.subscriber.decrypt(f.sent[0]!.body);
    expect(plaintext.length).toBeLessThan(1024);
    const payload = PushPayloadSchema.parse(JSON.parse(new TextDecoder().decode(plaintext)));
    expect(payload.tag).toBe(big);
    expect(f.sent[0]!.body.length).toBeLessThan(4096);
  });

  it("truncates names to 64 characters", async () => {
    const w = await createWorld();
    const f = fakeFetch();
    await notifyWith(w, bodyFor(w, { project: "p".repeat(100), desktopName: "d".repeat(100) }), f);
    expect(await plain(w, f.sent[0]!)).toMatchObject({ t: "d".repeat(64), b: `${"p".repeat(64)}: Finished` });
  });
});

describe("recipients", () => {
  it("a device that is not in deviceIds gets nothing", async () => {
    const w = await createWorld();
    const other = await createDevice(w.userId);
    await setGrant(other, w.desktopId, "confirmed");
    await insertSubscription(w.userId, other, await newSubscriber());
    const f = fakeFetch();
    expect(await notifyWith(w, bodyFor(w), f)).toEqual({ ok: true, sent: 1 });
    expect(f.sent).toHaveLength(1);
  });

  it("a pending grant, a revoked grant, a revoked device, another user's device and a grant on another desktop get nothing", async () => {
    const w = await createWorld();
    const pending = await createDevice(w.userId);
    await setGrant(pending, w.desktopId, "pending");
    const revokedGrant = await createDevice(w.userId);
    await setGrant(revokedGrant, w.desktopId, "revoked");
    const revokedDevice = await createDevice(w.userId, { revoked: true });
    await setGrant(revokedDevice, w.desktopId, "confirmed");
    const stranger = await createTestUser(env);
    const foreign = await createDevice(stranger.id);
    await setGrant(foreign, w.desktopId, "confirmed");
    const otherDesktop = await createTestDesktop(env, w.userId);
    const wrongDesktop = await createDevice(w.userId);
    await setGrant(wrongDesktop, otherDesktop, "confirmed");
    const noGrant = await createDevice(w.userId);
    const ids = [pending, revokedGrant, revokedDevice, foreign, wrongDesktop, noGrant];
    const own = [pending, revokedGrant, revokedDevice, wrongDesktop, noGrant];
    for (const id of ids) await insertSubscription(id === foreign ? stranger.id : w.userId, id, await newSubscriber());
    expect(own).toHaveLength(5);

    const f = fakeFetch();
    const result = await notifyWith(w, bodyFor(w, { deviceIds: ids }), f);
    expect(result).toEqual({ ok: true, sent: 0 });
    expect(f.sent).toHaveLength(0);
  });

  it("dedupes deviceIds, rejects more than 50 and caps recipients per notify", async () => {
    const w = await createWorld();
    const f = fakeFetch();
    expect(await notifyWith(w, bodyFor(w, { deviceIds: [w.deviceId, w.deviceId, w.deviceId] }), f)).toEqual({ ok: true, sent: 1 });
    const tooMany = Array.from({ length: 51 }, (_, i) => `d-${i}`);
    expect(await notifyWith(w, bodyFor(w, { deviceIds: tooMany }), f)).toMatchObject({ ok: false, status: 400 });
    expect(await notifyWith(w, bodyFor(w, { deviceIds: Array.from({ length: 50 }, (_, i) => `d-${i}`) }), f)).toEqual({ ok: true, sent: 0 });

    const many = await createWorld();
    for (let i = 0; i < 24; i++) await insertSubscription(many.userId, many.deviceId, await newSubscriber());
    const f2 = fakeFetch();
    const res = await notifyWith(many, bodyFor(many), f2);
    expect(res).toEqual({ ok: true, sent: MAX_RECIPIENTS });
    expect(f2.sent).toHaveLength(MAX_RECIPIENTS);
  });

  it("validates sessionId and monoId", async () => {
    const w = await createWorld();
    const f = fakeFetch();
    for (const bad of [{ sessionId: "a b" }, { sessionId: "a/b" }, { sessionId: "" }, { sessionId: "x".repeat(129) }, { monoId: "../x" }, { monoId: "m?x" }, { monoId: "" }]) {
      expect(await notifyWith(w, bodyFor(w, bad), f), JSON.stringify(bad)).toMatchObject({ ok: false, status: 400 });
    }
    expect(f.sent).toHaveLength(0);
    expect(await notifyWith(w, bodyFor(w, { category: "nope" }), f)).toMatchObject({ ok: false, status: 400 });
  });
});

describe("headers", () => {
  it("sends TTL, Urgency, Topic, Content-Encoding, Content-Type and the VAPID Authorization", async () => {
    const w = await createWorld();
    const vapid = await vapidEnv();
    const f = fakeFetch();
    const sessionId = sid();
    await notifyWith(w, bodyFor(w, { sessionId, category: "agentInput", status: "needs_input" }), f, { vapid });
    await notifyWith(w, bodyFor(w, { category: "agentFinished" }), f, { vapid });
    const [high, normal] = f.sent as [SentRequest, SentRequest];
    expect(high.url).toBe(w.endpoint);
    expect(high.headers["ttl"]).toBe("86400");
    expect(high.headers["urgency"]).toBe("high");
    expect(normal.headers["urgency"]).toBe("normal");
    expect(high.headers["topic"]).toBe(await topicFor(sessionId));
    expect(high.headers["topic"]).toMatch(/^[A-Za-z0-9_-]{32}$/);
    expect(high.headers["content-encoding"]).toBe("aes128gcm");
    expect(high.headers["content-type"]).toBe("application/octet-stream");
    expect(high.headers["authorization"]).toMatch(new RegExp(`^vapid t=[\\w-]+\\.[\\w-]+\\.[\\w-]+, k=${vapid.current.publicKey}$`));
  });
});

describe("push service answers", () => {
  it("404 and 410 delete the subscription", async () => {
    for (const status of [404, 410]) {
      const w = await createWorld();
      const f = fakeFetch(status);
      expect(await notifyWith(w, bodyFor(w), f)).toEqual({ ok: true, sent: 0 });
      expect(await subscriptionRow(w.subscriptionId), String(status)).toBeNull();
    }
  });

  it("other errors and network failures count a failure; the fifth deletes the row", async () => {
    const w = await createWorld();
    const f = fakeFetch(500);
    await notifyWith(w, bodyFor(w), f);
    expect((await subscriptionRow(w.subscriptionId))?.failures).toBe(1);
    const thrower = { sent: [] as SentRequest[], fetch: () => Promise.reject(new Error("offline")) };
    await notifyWith(w, bodyFor(w), thrower);
    expect((await subscriptionRow(w.subscriptionId))?.failures).toBe(2);
    await env.DB.prepare("UPDATE push_subscriptions SET failures = 4 WHERE id = ?").bind(w.subscriptionId).run();
    await notifyWith(w, bodyFor(w), f);
    expect(await subscriptionRow(w.subscriptionId)).toBeNull();
  });

  it("429 backs off that endpoint for Retry-After without counting a failure", async () => {
    const w = await createWorld();
    const limited = fakeFetch(() => new Response(null, { status: 429, headers: { "Retry-After": "30" } }));
    expect(await notifyWith(w, bodyFor(w), limited, { nowMs: NOW })).toEqual({ ok: true, sent: 0 });
    expect(limited.sent).toHaveLength(1);
    expect((await subscriptionRow(w.subscriptionId))?.failures).toBe(0);
    const during = fakeFetch();
    await notifyWith(w, bodyFor(w), during, { nowMs: NOW + 29_000 });
    expect(during.sent).toHaveLength(0);
    await notifyWith(w, bodyFor(w), during, { nowMs: NOW + 31_000 });
    expect(during.sent).toHaveLength(1);
  });

  it("a success writes nothing while failures is 0 and last_success_at is recent", async () => {
    const w = await createWorld();
    const recent = Math.floor(NOW / 1000) - 100;
    await env.DB.prepare("UPDATE push_subscriptions SET last_success_at = ? WHERE id = ?").bind(recent, w.subscriptionId).run();
    const counting = countingDb();
    const vapid = await vapidEnv();
    const result = await runNotify(overlayEnv({ ...Object.fromEntries(Object.entries(vapid.env)), DB: counting.db }), principal(w), bodyFor(w), { fetch: fakeFetch().fetch, nowMs: () => NOW });
    expect(result).toEqual({ ok: true, sent: 1 });
    // A clean success writes nothing: no subscription statement and no audit row.
    expect(counting.batches).toBe(0);
    expect((await subscriptionRow(w.subscriptionId))?.last_success_at).toBe(recent);
  });

  it("a success writes when failures > 0 or last_success_at is older than 24 h", async () => {
    const failing = await createWorld();
    await env.DB.prepare("UPDATE push_subscriptions SET failures = 2, last_success_at = ? WHERE id = ?").bind(Math.floor(NOW / 1000) - 10, failing.subscriptionId).run();
    await notifyWith(failing, bodyFor(failing), fakeFetch());
    expect(await subscriptionRow(failing.subscriptionId)).toMatchObject({ failures: 0, last_success_at: Math.floor(NOW / 1000) });

    const stale = await createWorld();
    await env.DB.prepare("UPDATE push_subscriptions SET last_success_at = ? WHERE id = ?").bind(Math.floor(NOW / 1000) - 90_000, stale.subscriptionId).run();
    await notifyWith(stale, bodyFor(stale), fakeFetch());
    expect((await subscriptionRow(stale.subscriptionId))?.last_success_at).toBe(Math.floor(NOW / 1000));
  });

  it("writes every change of one notify in a single db.batch", async () => {
    const w = await createWorld();
    const gone = await insertSubscription(w.userId, w.deviceId, await newSubscriber(), { endpoint: "https://fcm.googleapis.com/fcm/send/gone" });
    const broken = await insertSubscription(w.userId, w.deviceId, await newSubscriber(), { endpoint: "https://fcm.googleapis.com/fcm/send/broken" });
    const f = fakeFetch((url) => new Response(null, { status: url.endsWith("/gone") ? 410 : url.endsWith("/broken") ? 500 : 201 }));
    const counting = countingDb();
    const vapid = await vapidEnv();
    const result = await runNotify(overlayEnv({ ...Object.fromEntries(Object.entries(vapid.env)), DB: counting.db }), principal(w), bodyFor(w), { fetch: f.fetch, nowMs: () => NOW });
    expect(result).toEqual({ ok: true, sent: 1 });
    expect(counting.batches).toBe(1);
    expect(await subscriptionRow(gone)).toBeNull();
    expect((await subscriptionRow(broken))?.failures).toBe(1);
    expect((await subscriptionRow(w.subscriptionId))?.last_success_at).toBe(Math.floor(NOW / 1000));
  });
});

/** A DB wrapper that counts `batch` calls. */
function countingDb() {
  const state = { batches: 0, sizes: [] as number[] };
  const db = {
    prepare: (sql: string) => env.DB.prepare(sql),
    batch: (statements: D1PreparedStatement[]) => {
      state.batches++;
      state.sizes.push(statements.length);
      return env.DB.batch(statements);
    },
  } as unknown as D1Database;
  return { db, get batches() { return state.batches; }, get sizes() { return state.sizes; } };
}

describe("dedupe per session", () => {
  it("skips the same desktop, session and status within 10 s, but not another status or a later notice", async () => {
    const w = await createWorld();
    const f = fakeFetch();
    const body = bodyFor(w);
    expect(await notifyWith(w, body, f, { nowMs: NOW })).toEqual({ ok: true, sent: 1 });
    expect(await notifyWith(w, body, f, { nowMs: NOW + 9_000 })).toEqual({ ok: true, sent: 0 });
    expect(f.sent).toHaveLength(1);
    expect(await notifyWith(w, { ...body, status: "needs_input", category: "agentInput" }, f, { nowMs: NOW + 9_000 })).toEqual({ ok: true, sent: 1 });
    expect(await notifyWith(w, body, f, { nowMs: NOW + 10_001 })).toEqual({ ok: true, sent: 1 });
  });
});

describe("token scope over HTTP", () => {
  async function post(_w: World, token: string | null, body: unknown, testEnv = overlayEnv({})) {
    return callWithEnv(testEnv, "/api/push/notify", { method: "POST", headers: { "Content-Type": "application/json", ...(token ? { Authorization: `Bearer ${token}` } : {}) }, body: JSON.stringify(body) });
  }

  it("refuses a missing token, a token without the push scope and a revoked token", async () => {
    const w = await createWorld();
    expect((await post(w, null, bodyFor(w))).status).toBe(401);
    expect((await post(w, await createToken(w.desktopId, { scope: "relay grants" }), bodyFor(w))).status).toBe(403);
    expect((await post(w, await createToken(w.desktopId, { revoked: true }), bodyFor(w))).status).toBe(403);
  });

  it("with a push token sends through the global fetch, strips unknown keys, and answers {sent}", async () => {
    const w = await createWorld();
    const vapid = await vapidEnv();
    const requests: string[] = [];
    vi.stubGlobal("fetch", async (url: string) => {
      requests.push(url);
      return new Response(null, { status: 201 });
    });
    const res = await post(w, w.token, { ...bodyFor(w), title: "SECRET" }, vapid.env);
    expect(res.status).toBe(200);
    expect(await res.json()).toEqual({ sent: 1 });
    expect(requests).toEqual([w.endpoint]);
  });

  it("answers 400 for invalid input and 503 when the VAPID keys are missing or invalid, without sending", async () => {
    const w = await createWorld();
    const requests: string[] = [];
    vi.stubGlobal("fetch", async (url: string) => {
      requests.push(url);
      return new Response(null, { status: 201 });
    });
    expect((await post(w, w.token, { ...bodyFor(w), sessionId: "a b" }, (await vapidEnv()).env)).status).toBe(400);
    const res = await post(w, w.token, bodyFor(w), overlayEnv({ VAPID_PUBLIC_KEY: "REPLACE_WITH_VAPID_PUBLIC_KEY_BASE64URL", VAPID_PRIVATE_KEY: "test-vapid-private-key" }));
    expect(res.status).toBe(503);
    expect(await res.json()).toMatchObject({ code: "internal" });
    expect(requests).toHaveLength(0);
  });
});

describe("VAPID rotation, dual key", () => {
  it("signs with the key that matches the subscription's vapid_key_id and skips (without deleting) unknown key ids", async () => {
    const w = await createWorld();
    const vapid = await vapidEnv({ withPrevious: true });
    const oldSub = await insertSubscription(w.userId, w.deviceId, w.subscriber, { endpoint: "https://fcm.googleapis.com/fcm/send/old", vapidKeyId: "old" });
    const unknownSub = await insertSubscription(w.userId, w.deviceId, w.subscriber, { endpoint: "https://fcm.googleapis.com/fcm/send/unknown", vapidKeyId: "retired" });
    const f = fakeFetch();
    const result = await runNotify(vapid.env, principal(w), bodyFor(w), { fetch: f.fetch, nowMs: () => NOW });
    expect(result).toEqual({ ok: true, sent: 2 });
    expect(f.sent.map((r) => r.url).sort()).toEqual([w.endpoint, "https://fcm.googleapis.com/fcm/send/old"].sort());
    for (const request of f.sent) {
      const isOld = request.url.endsWith("/old");
      const expected = isOld ? vapid.previous : vapid.current;
      const match = /^vapid t=([\w-]+)\.([\w-]+)\.([\w-]+), k=([\w-]+)$/.exec(request.headers["authorization"] ?? "") as RegExpExecArray;
      expect(match[4]).toBe(expected.publicKey);
      const key = await crypto.subtle.importKey("raw", expected.publicRaw, { name: "ECDSA", namedCurve: "P-256" }, false, ["verify"]);
      expect(await crypto.subtle.verify({ name: "ECDSA", hash: "SHA-256" }, key, fromBase64Url(match[3]!), new TextEncoder().encode(`${match[1]}.${match[2]}`))).toBe(true);
    }
    expect(await subscriptionRow(unknownSub)).not.toBeNull();
    expect(await subscriptionRow(oldSub)).not.toBeNull();
  });

  it("skips a subscription on the previous key id once the previous vars are removed", async () => {
    const w = await createWorld({ vapidKeyId: "old" });
    const f = fakeFetch();
    expect(await notifyWith(w, bodyFor(w), f)).toEqual({ ok: true, sent: 0 });
    expect(f.sent).toHaveLength(0);
    expect(await subscriptionRow(w.subscriptionId)).not.toBeNull();
  });
});

describe("message content", () => {
  const SECRET = "the-secret-reply-text-7f3a";

  it("puts the title and the preview into the encrypted payload", async () => {
    const w = await createWorld();
    const f = fakeFetch();
    await notifyWith(w, bodyFor(w, { title: "Bach Tieu Thuan", preview: SECRET }), f);
    expect(await plain(w, f.sent[0]!)).toMatchObject({ t: "Bach Tieu Thuan", b: SECRET });
    // Encrypted end to end: the bytes that reach the push service do not show the text.
    expect(new TextDecoder("latin1").decode(f.sent[0]!.body)).not.toContain(SECRET);
    expect(JSON.stringify(f.sent[0]!.headers)).not.toContain(SECRET);
    expect(f.sent[0]!.url).not.toContain(SECRET);
  });

  it("falls back to the generic text without content or with only half of it", async () => {
    const w = await createWorld();
    const f = fakeFetch();
    await notifyWith(w, bodyFor(w), f);
    await notifyWith(w, bodyFor(w, { title: "Name" }), f);
    await notifyWith(w, bodyFor(w, { preview: "   " , title: "Name" }), f);
    for (const request of f.sent) expect(await plain(w, request)).toMatchObject({ b: "Alpha: Finished" });
  });

  it("collapses lines and cuts a long preview to 180 characters, and stays under 3 KiB", async () => {
    const w = await createWorld();
    const f = fakeFetch();
    const big = `${":".repeat(100)}${"x".repeat(28)}`;
    await notifyWith(w, bodyFor(w, { title: "\u{1F600}".repeat(500), preview: `a\n\n  b ${"\u{1F600}".repeat(1000)}`, sessionId: big, monoId: big, habitRun: true }), f);
    const plaintext = await w.subscriber.decrypt(f.sent[0]!.body);
    expect(plaintext.length).toBeLessThan(3072);
    const payload = PushPayloadSchema.parse(JSON.parse(new TextDecoder().decode(plaintext)));
    expect(payload.b.startsWith("a b ")).toBe(true);
    expect(Array.from(payload.b)).toHaveLength(180);
    expect(payload.b.endsWith("…")).toBe(true);
    expect(Array.from(payload.t)).toHaveLength(64);
    expect(f.sent[0]!.body.length).toBeLessThan(4096);
  });

  it("rejects an oversized preview in the request", async () => {
    const w = await createWorld();
    const f = fakeFetch();
    expect(await notifyWith(w, bodyFor(w, { title: "n", preview: "x".repeat(5000) }), f)).toMatchObject({ ok: false, status: 400 });
    expect(f.sent).toHaveLength(0);
  });

  it("never logs or stores the content, even when the push service fails", async () => {
    const w = await createWorld();
    const lines: string[] = [];
    for (const method of ["log", "info", "warn", "error", "debug"] as const) {
      vi.spyOn(console, method).mockImplementation((...args: unknown[]) => void lines.push(args.map(String).join(" ")));
    }
    try {
      await notifyWith(w, bodyFor(w, { title: "Private Name", preview: SECRET }), fakeFetch(500));
      await notifyWith(w, bodyFor(w, { title: "Private Name", preview: SECRET }), fakeFetch(201));
    } finally {
      vi.restoreAllMocks();
    }
    expect(lines.length).toBeGreaterThan(0);
    expect(lines.join("\n")).not.toContain(SECRET);
    expect(lines.join("\n")).not.toContain("Private Name");
    const dump = JSON.stringify([
      (await env.DB.prepare("SELECT * FROM audit_log").all()).results,
      (await env.DB.prepare("SELECT * FROM push_subscriptions").all()).results,
      (await env.DB.prepare("SELECT * FROM push_prefs").all()).results,
    ]);
    expect(dump).not.toContain(SECRET);
    expect(dump).not.toContain("Private Name");
  });
});
