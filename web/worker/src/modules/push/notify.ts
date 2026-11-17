// `POST /api/push/notify` (P-02): the desktop asks the Worker to push to the listed devices (architecture.md 8).
import type { Handler } from "hono";
import { PushNotifyBody, PushNotifyResponse } from "@monoptah/protocol";
import { apiError, respond, type AppEnv } from "../../http.ts";
import type { Env } from "../../env.ts";
import { requireDesktopToken, type DesktopPrincipal } from "../desktops/token.ts";
import { buildPayload, SAFE_ID, topicFor } from "./payload.ts";
import { currentKey, keyIsUsable, previousKey, vapidAuthorization, type VapidEnv, type VapidKeyConfig } from "./vapid.ts";
import { sendWebPush, type EncryptOptions, type PushFetch, type PushOutcome } from "./webpush.ts";

export const MAX_DEVICE_IDS = 50;
export const MAX_RECIPIENTS = 20;
export const MAX_FAILURES = 5;
export const DEDUPE_WINDOW_MS = 10_000;
export const SUCCESS_WRITE_INTERVAL_SECONDS = 86_400;
const MEMORY_LIMIT = 1000;

/** Desktop name and the owner's prefs in one lookup (primary keys). No prefs row: push is off. */
export const DESKTOP_PREFS_SQL =
  "SELECT d.name, p.enabled, p.categories FROM desktops d LEFT JOIN push_prefs p ON p.desktop_id = d.id AND p.user_id = d.user_id WHERE d.id = ? AND d.user_id = ?";

/** Subscriptions of the listed devices that belong to the owner, are not revoked and hold a confirmed grant on this desktop. */
export function recipientsSql(deviceCount: number): string {
  const marks = Array.from({ length: deviceCount }, () => "?").join(", ");
  return `SELECT s.id, s.endpoint, s.p256dh, s.auth, s.vapid_key_id, s.failures, s.last_success_at
  FROM push_subscriptions s
  JOIN devices dv ON dv.id = s.device_id
  JOIN device_grants g ON g.device_id = dv.id AND g.desktop_id = ?
  WHERE s.device_id IN (${marks}) AND s.user_id = ? AND dv.user_id = ? AND dv.revoked_at IS NULL AND g.state = 'confirmed'
  LIMIT ${MAX_RECIPIENTS}`;
}
export const DELETE_SUBSCRIPTION_BY_ID_SQL = "DELETE FROM push_subscriptions WHERE id = ?";
export const MARK_SUCCESS_SQL = "UPDATE push_subscriptions SET last_success_at = ?, failures = 0 WHERE id = ?";
export const BUMP_FAILURES_SQL = "UPDATE push_subscriptions SET failures = failures + 1 WHERE id = ?";

interface Recipient {
  id: string;
  endpoint: string;
  p256dh: string;
  auth: string;
  vapid_key_id: string;
  failures: number;
  last_success_at: number | null;
}

/** Per-isolate state, best effort: a new isolate starts empty. Bounded. */
const recentNotices = new Map<string, number>();
const backoffUntil = new Map<string, number>();
let warnedKeys = false;

function remember(map: Map<string, number>, key: string, value: number, nowMs: number): void {
  if (map.size >= MEMORY_LIMIT) {
    for (const [k, v] of map) if (v <= nowMs) map.delete(k);
    if (map.size >= MEMORY_LIMIT) map.delete(map.keys().next().value as string);
  }
  map.set(key, value);
}

/** Test hook: empties dedupe, backoff and the warn-once flag. */
export function resetNotifyState(): void {
  recentNotices.clear();
  backoffUntil.clear();
  warnedKeys = false;
}

export interface NotifyDeps {
  fetch: PushFetch;
  nowMs: () => number;
  /** Salt and ephemeral key override, for tests. */
  encrypt?: EncryptOptions;
}

export const defaultDeps: NotifyDeps = {
  // Resolved on every call, so a test can replace `globalThis.fetch`.
  fetch: (input, init) => globalThis.fetch(input, init),
  nowMs: () => Date.now(),
};

export type NotifyResult = { ok: true; sent: number } | { ok: false; status: 400 | 503; message: string };

function parseCategories(raw: string | null): Record<string, boolean> {
  try {
    const value: unknown = JSON.parse(raw ?? "{}");
    return typeof value === "object" && value !== null ? (value as Record<string, boolean>) : {};
  } catch {
    return {};
  }
}

/** The whole notify flow for an authenticated desktop. `body` is the raw JSON. */
export async function runNotify(env: Env, principal: DesktopPrincipal, rawBody: unknown, deps: NotifyDeps = defaultDeps): Promise<NotifyResult> {
  const parsed = PushNotifyBody.safeParse(rawBody);
  if (!parsed.success) return { ok: false, status: 400, message: "Invalid notify request" };
  const body = parsed.data;
  if (!SAFE_ID.test(body.sessionId) || (body.monoId !== undefined && !SAFE_ID.test(body.monoId))) return { ok: false, status: 400, message: "Invalid id" };
  const deviceIds = [...new Set(body.deviceIds)];
  if (deviceIds.length === 0) return { ok: true, sent: 0 };
  if (deviceIds.length > MAX_DEVICE_IDS) return { ok: false, status: 400, message: "Too many devices" };

  const nowMs = deps.nowMs();
  const desktop = await env.DB.prepare(DESKTOP_PREFS_SQL).bind(principal.desktopId, principal.userId).first<{ name: string; enabled: number | null; categories: string | null }>();
  if (!desktop || desktop.enabled !== 1 || parseCategories(desktop.categories)[body.category] !== true) return { ok: true, sent: 0 };

  const noticeKey = `${principal.desktopId}|${body.sessionId}|${body.status}`;
  const last = recentNotices.get(noticeKey);
  if (last !== undefined && nowMs - last < DEDUPE_WINDOW_MS) return { ok: true, sent: 0 };

  const recipients = (
    await env.DB.prepare(recipientsSql(deviceIds.length)).bind(principal.desktopId, ...deviceIds, principal.userId, principal.userId).all<Recipient>()
  ).results;
  if (recipients.length === 0) return { ok: true, sent: 0 };

  const vapidEnv = env as unknown as VapidEnv;
  const current = currentKey(vapidEnv);
  if (!(await keyIsUsable(current)) || !vapidEnv.VAPID_SUBJECT) {
    if (!warnedKeys) {
      warnedKeys = true;
      console.warn(JSON.stringify({ event: "push.vapid_unusable", desktopId: principal.desktopId }));
    }
    return { ok: false, status: 503, message: "Push is not configured" };
  }
  remember(recentNotices, noticeKey, nowMs, nowMs);

  const previous = previousKey(vapidEnv);
  const previousUsable = await keyIsUsable(previous);
  const keyFor = (keyId: string): VapidKeyConfig | null => {
    if (current && keyId === current.keyId) return current;
    if (previous && previousUsable && keyId === previous.keyId) return previous;
    return null;
  };

  const payload = buildPayload({
    desktopId: principal.desktopId,
    desktopName: body.desktopName ?? desktop.name,
    project: body.project,
    status: body.status,
    sessionId: body.sessionId,
    monoId: body.monoId,
    habitRun: body.habitRun,
  });
  const topic = await topicFor(body.sessionId);
  const urgency = body.category === "agentInput" ? "high" : "normal";
  const subject = vapidEnv.VAPID_SUBJECT;

  let skippedKey = 0;
  const jobs: { row: Recipient; send: Promise<PushOutcome> }[] = [];
  for (const row of recipients) {
    const key = keyFor(row.vapid_key_id);
    if (!key) {
      skippedKey++;
      continue;
    }
    if ((backoffUntil.get(row.endpoint) ?? 0) > nowMs) continue;
    jobs.push({
      row,
      send: vapidAuthorization(key, subject, row.endpoint, Math.floor(nowMs / 1000)).then((authorization) =>
        sendWebPush(deps.fetch, row, payload, { authorization, urgency, topic }, nowMs, deps.encrypt),
      ),
    });
  }
  const settled = await Promise.allSettled(jobs.map((job) => job.send));

  const nowSec = Math.floor(nowMs / 1000);
  const statements: D1PreparedStatement[] = [];
  let sent = 0;
  settled.forEach((result, i) => {
    const row = jobs[i]!.row;
    const outcome: PushOutcome = result.status === "fulfilled" ? result.value : { kind: "failed" };
    switch (outcome.kind) {
      case "ok":
        sent++;
        if (row.failures > 0 || row.last_success_at === null || nowSec - row.last_success_at >= SUCCESS_WRITE_INTERVAL_SECONDS) {
          statements.push(env.DB.prepare(MARK_SUCCESS_SQL).bind(nowSec, row.id));
        }
        break;
      case "gone":
        statements.push(env.DB.prepare(DELETE_SUBSCRIPTION_BY_ID_SQL).bind(row.id));
        break;
      case "backoff":
        remember(backoffUntil, row.endpoint, nowMs + outcome.retryAfterSeconds * 1000, nowMs);
        break;
      case "failed":
        statements.push(env.DB.prepare(row.failures + 1 >= MAX_FAILURES ? DELETE_SUBSCRIPTION_BY_ID_SQL : BUMP_FAILURES_SQL).bind(row.id));
        break;
    }
  });
  if (statements.length > 0) await env.DB.batch(statements);
  console.log(JSON.stringify({ event: "push.notify", desktopId: principal.desktopId, recipients: recipients.length, sent, skippedKey, writes: statements.length }));
  return { ok: true, sent };
}

/** `POST /api/push/notify` (token scope `push`). */
export const notify: Handler<AppEnv> = async (c) => {
  const auth = await requireDesktopToken(c, "push");
  if (auth instanceof Response) return auth;
  const result = await runNotify(c.env, auth, await c.req.json().catch(() => null));
  if (!result.ok) return result.status === 503 ? apiError(c, 503, "internal", result.message) : apiError(c, 400, "invalid", result.message);
  return respond(c, PushNotifyResponse, { sent: result.sent });
};
