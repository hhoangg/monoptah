// `POST /api/push/notify` (P-02): the desktop asks the Worker to push to the listed devices (architecture.md 8).
import type { Handler } from "hono";
import { PushNotifyBody, PushNotifyResponse } from "@monoptah/protocol";
import { apiError, respond, type AppEnv } from "../../http.ts";
import type { Env } from "../../env.ts";
import { requireDesktopToken, type DesktopPrincipal } from "../desktops/token.ts";
import { auditStatement } from "../audit/audit.ts";
import { buildPayload, SAFE_ID, targetKey, topicFor } from "./payload.ts";
import { resolvePrefs, type PrefsColumns } from "./prefs.ts";
import { currentKey, keyIsUsable, previousKey, vapidAuthorization, type VapidEnv, type VapidKeyConfig } from "./vapid.ts";
import { sendWebPush, type EncryptOptions, type PushFetch, type PushOutcome } from "./webpush.ts";

export const MAX_DEVICE_IDS = 50;
export const MAX_RECIPIENTS = 20;
export const MAX_FAILURES = 5;
export const DEDUPE_WINDOW_MS = 10_000;
export const SUCCESS_WRITE_INTERVAL_SECONDS = 86_400;
const MEMORY_LIMIT = 1000;

/** Desktop name and the owner's prefs in one lookup (primary keys). No prefs row (both columns null): the defaults, push on (`resolvePrefs`). */
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

export type PushTestOutcome = PushOutcome["kind"] | "skipped_key";

export interface Recipient {
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

export type NotifyOutcome = "sent" | "prefs_off" | "category_off" | "deduped" | "no_recipients" | "no_devices" | "invalid" | "vapid_unusable" | "write_failed";

/** What one delivery round did, by subscription. `statuses` are the HTTP codes the push services answered (0: no answer). */
export interface DeliveryReport {
  results: { outcome: PushTestOutcome; status: number }[];
  sent: number;
  gone: number;
  failed: number;
  backoff: number;
  skippedKey: number;
  statuses: number[];
  /** Subscription changes to commit (one `db.batch` with the caller's other statements). */
  statements: D1PreparedStatement[];
}

/** One structured, metadata-only line per attempt: never endpoints, keys, tokens or names. */
export function logAttempt(event: "push.notify" | "push.test", fields: Record<string, unknown>, report?: Pick<DeliveryReport, "sent" | "gone" | "failed" | "backoff" | "skippedKey" | "statuses">, recipients = 0): void {
  console.log(
    JSON.stringify({
      event,
      ...fields,
      recipients,
      sent: report?.sent ?? 0,
      gone: report?.gone ?? 0,
      failed: report?.failed ?? 0,
      backoff: report?.backoff ?? 0,
      skippedKey: report?.skippedKey ?? 0,
      statuses: report?.statuses ?? [],
    }),
  );
}

/** The signing keys of this request, or null when the current key or the subject is unusable. */
export async function prepareVapid(env: Env): Promise<{ keyFor: (keyId: string) => VapidKeyConfig | null; subject: string } | null> {
  const vapidEnv = env as unknown as VapidEnv;
  const current = currentKey(vapidEnv);
  if (!(await keyIsUsable(current)) || !vapidEnv.VAPID_SUBJECT) return null;
  const previous = previousKey(vapidEnv);
  const previousUsable = await keyIsUsable(previous);
  return {
    subject: vapidEnv.VAPID_SUBJECT,
    keyFor: (keyId) => {
      if (current && keyId === current.keyId) return current;
      if (previous && previousUsable && keyId === previous.keyId) return previous;
      return null;
    },
  };
}

/** Warns once per isolate that the VAPID keys cannot sign. */
export function warnVapidUnusable(desktopId: string | null): void {
  if (warnedKeys) return;
  warnedKeys = true;
  console.warn(JSON.stringify({ event: "push.vapid_unusable", desktopId }));
}

export interface DeliveryInput {
  rows: Recipient[];
  payload: Uint8Array;
  urgency: "high" | "normal";
  topic: string;
  vapid: NonNullable<Awaited<ReturnType<typeof prepareVapid>>>;
  /** Skip endpoints that answered 429 recently. A manual test ignores the backoff. */
  honorBackoff: boolean;
}

/** Signs, encrypts and sends to each row in parallel, then turns the answers into D1 statements (not run here). */
export async function deliver(env: Env, input: DeliveryInput, deps: NotifyDeps, nowMs: number): Promise<DeliveryReport> {
  const report: DeliveryReport = { results: [], sent: 0, gone: 0, failed: 0, backoff: 0, skippedKey: 0, statuses: [], statements: [] };
  const jobs: { row: Recipient; send: Promise<PushOutcome> }[] = [];
  for (const row of input.rows) {
    const key = input.vapid.keyFor(row.vapid_key_id);
    if (!key) {
      report.skippedKey++;
      report.results.push({ outcome: "skipped_key", status: 0 });
      continue;
    }
    if (input.honorBackoff && (backoffUntil.get(row.endpoint) ?? 0) > nowMs) continue;
    jobs.push({
      row,
      send: vapidAuthorization(key, input.vapid.subject, row.endpoint, Math.floor(nowMs / 1000)).then((authorization) =>
        sendWebPush(deps.fetch, row, input.payload, { authorization, urgency: input.urgency, topic: input.topic }, nowMs, deps.encrypt),
      ),
    });
  }
  const settled = await Promise.allSettled(jobs.map((job) => job.send));

  const nowSec = Math.floor(nowMs / 1000);
  settled.forEach((result, i) => {
    const row = jobs[i]!.row;
    const outcome: PushOutcome = result.status === "fulfilled" ? result.value : { kind: "failed", status: 0 };
    report.results.push({ outcome: outcome.kind, status: outcome.status });
    report.statuses.push(outcome.status);
    switch (outcome.kind) {
      case "ok":
        report.sent++;
        if (row.failures > 0 || row.last_success_at === null || nowSec - row.last_success_at >= SUCCESS_WRITE_INTERVAL_SECONDS) {
          report.statements.push(env.DB.prepare(MARK_SUCCESS_SQL).bind(nowSec, row.id));
        }
        break;
      case "gone":
        report.gone++;
        report.statements.push(env.DB.prepare(DELETE_SUBSCRIPTION_BY_ID_SQL).bind(row.id));
        break;
      case "backoff":
        report.backoff++;
        remember(backoffUntil, row.endpoint, nowMs + outcome.retryAfterSeconds * 1000, nowMs);
        break;
      case "failed":
        report.failed++;
        report.statements.push(env.DB.prepare(row.failures + 1 >= MAX_FAILURES ? DELETE_SUBSCRIPTION_BY_ID_SQL : BUMP_FAILURES_SQL).bind(row.id));
        break;
    }
  });
  return report;
}

/** The whole notify flow for an authenticated desktop. `body` is the raw JSON. Every call writes one `push.notify` log line; an audit row only when something went wrong. */
export async function runNotify(env: Env, principal: DesktopPrincipal, rawBody: unknown, deps: NotifyDeps = defaultDeps): Promise<NotifyResult> {
  const log = (outcome: NotifyOutcome, category: string | null, report?: DeliveryReport, recipients = 0): void =>
    logAttempt("push.notify", { desktopId: principal.desktopId, category, outcome }, report, recipients);

  const parsed = PushNotifyBody.safeParse(rawBody);
  if (!parsed.success) {
    log("invalid", null);
    return { ok: false, status: 400, message: "Invalid notify request" };
  }
  const body = parsed.data;
  // An empty sessionId counts as absent only next to a monoId (new desktops omit it; old ones never send it empty).
  const sessionId = body.sessionId === "" && body.monoId !== undefined ? undefined : body.sessionId;
  const idsOk =
    (sessionId !== undefined || body.monoId !== undefined) &&
    !(body.habitRun === true && sessionId === undefined) &&
    (sessionId === undefined || SAFE_ID.test(sessionId)) &&
    (body.monoId === undefined || SAFE_ID.test(body.monoId));
  if (!idsOk) {
    log("invalid", body.category);
    return { ok: false, status: 400, message: "Invalid id" };
  }
  const deviceIds = [...new Set(body.deviceIds)];
  if (deviceIds.length === 0) {
    log("no_devices", body.category);
    return { ok: true, sent: 0 };
  }
  if (deviceIds.length > MAX_DEVICE_IDS) {
    log("invalid", body.category);
    return { ok: false, status: 400, message: "Too many devices" };
  }

  const nowMs = deps.nowMs();
  const desktop = await env.DB.prepare(DESKTOP_PREFS_SQL).bind(principal.desktopId, principal.userId).first<{ name: string } & PrefsColumns>();
  if (!desktop) {
    log("invalid", body.category);
    return { ok: true, sent: 0 };
  }
  const prefs = resolvePrefs(desktop);
  if (!prefs.enabled) {
    log("prefs_off", body.category);
    return { ok: true, sent: 0 };
  }
  if (!prefs.categories[body.category]) {
    log("category_off", body.category);
    return { ok: true, sent: 0 };
  }

  const target = targetKey({ sessionId, monoId: body.monoId });
  const noticeKey = `${principal.desktopId}|${target}|${body.status}`;
  const last = recentNotices.get(noticeKey);
  if (last !== undefined && nowMs - last < DEDUPE_WINDOW_MS) {
    log("deduped", body.category);
    return { ok: true, sent: 0 };
  }

  const recipients = (
    await env.DB.prepare(recipientsSql(deviceIds.length)).bind(principal.desktopId, ...deviceIds, principal.userId, principal.userId).all<Recipient>()
  ).results;
  if (recipients.length === 0) {
    log("no_recipients", body.category);
    return { ok: true, sent: 0 };
  }

  const vapid = await prepareVapid(env);
  if (!vapid) {
    warnVapidUnusable(principal.desktopId);
    log("vapid_unusable", body.category, undefined, recipients.length);
    return { ok: false, status: 503, message: "Push is not configured" };
  }
  remember(recentNotices, noticeKey, nowMs, nowMs);

  const payload = buildPayload({
    desktopId: principal.desktopId,
    desktopName: body.desktopName ?? desktop.name,
    project: body.project,
    status: body.status,
    sessionId,
    monoId: body.monoId,
    habitRun: body.habitRun,
  });
  const topic = await topicFor(target);
  const urgency = body.category === "agentInput" ? "high" : "normal";
  const report = await deliver(env, { rows: recipients, payload, urgency, topic, vapid, honorBackoff: true }, deps, nowMs);

  // Subscription changes, plus one audit row only when something went wrong (a failure, a gone endpoint, a 429
  // backoff, or no delivery at all). A clean success writes nothing beyond the occasional subscription update, so
  // the audit table and the D1 write budget do not grow with every notice; the log line covers every attempt.
  const statements = [...report.statements];
  const attention = report.failed > 0 || report.gone > 0 || report.backoff > 0 || (recipients.length > 0 && report.sent === 0);
  if (attention) {
    statements.push(
      auditStatement(env, {
        action: "push.notify",
        actorKind: "desktop",
        actorUserId: principal.userId,
        targetType: "desktop",
        targetId: principal.desktopId,
        meta: { desktopId: principal.desktopId, category: body.category, recipients: recipients.length, sent: report.sent, gone: report.gone, failed: report.failed, backoff: report.backoff },
      }),
    );
  }
  // The pushes are already out: a failed write must not turn into a 500, or the desktop retries and sends twice.
  let writeFailed = false;
  if (statements.length > 0) {
    try {
      await env.DB.batch(statements);
    } catch {
      writeFailed = true;
    }
  }
  log(writeFailed ? "write_failed" : "sent", body.category, report, recipients.length);
  return { ok: true, sent: report.sent };
}

/** `POST /api/push/notify` (token scope `push`). */
export const notify: Handler<AppEnv> = async (c) => {
  const auth = await requireDesktopToken(c, "push");
  if (auth instanceof Response) return auth;
  const result = await runNotify(c.env, auth, await c.req.json().catch(() => null));
  if (!result.ok) return result.status === 503 ? apiError(c, 503, "internal", result.message) : apiError(c, 400, "invalid", result.message);
  return respond(c, PushNotifyResponse, { sent: result.sent });
};
