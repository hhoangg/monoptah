// `POST /api/push/test`: sends one test notification to every subscription of the caller's own device, so the
// user can tell whether the whole chain works. It ignores prefs and dedupe and shares the sender and the
// bookkeeping (gone, failures, success) with notify.
import type { Handler } from "hono";
import { PushPayloadSchema, PushTestBody, PushTestResponse } from "@monoptah/protocol";
import { apiError, respond, type AppEnv } from "../../http.ts";
import type { Env } from "../../env.ts";
import { requireAuth } from "../../middleware/auth.ts";
import { auditStatement } from "../audit/audit.ts";
import { identifierHash } from "../../lib/ip.ts";
import { auditBreach, consume, LIMITS, rateLimitKey } from "../ratelimit/index.ts";
import { defaultDeps, deliver, logAttempt, MAX_RECIPIENTS, prepareVapid, warnVapidUnusable, type NotifyDeps, type Recipient } from "./notify.ts";
import { topicFor } from "./payload.ts";
import { OWN_DEVICE_SQL } from "./subscriptions.ts";

/** Subscriptions of one device of the owner. `user_id` uses an index; a device holds at most 4 anyway. */
export const DEVICE_SUBSCRIPTIONS_SQL = `SELECT id, endpoint, p256dh, auth, vapid_key_id, failures, last_success_at
  FROM push_subscriptions WHERE user_id = ? AND device_id = ? LIMIT ${MAX_RECIPIENTS}`;

const TEST_TAG = "monoptah-test";
const encoder = new TextEncoder();

type TestResponseBody = ReturnType<typeof PushTestResponse.parse>;
export type PushTestResult = { ok: true; body: TestResponseBody } | { ok: false; status: 404 | 503 };

/** Sends the test notification to `deviceId` of `userId`. Writes one `push.test` log line and one audit row. */
export async function runPushTest(env: Env, userId: string, deviceId: string, deps: NotifyDeps = defaultDeps): Promise<PushTestResult> {
  const device = await env.DB.prepare(OWN_DEVICE_SQL).bind(deviceId, userId).first();
  if (!device) return { ok: false, status: 404 };

  const rows = (await env.DB.prepare(DEVICE_SUBSCRIPTIONS_SQL).bind(userId, deviceId).all<Recipient>()).results;
  const nowMs = deps.nowMs();
  let report: Awaited<ReturnType<typeof deliver>> | undefined;
  if (rows.length > 0) {
    const vapid = await prepareVapid(env);
    if (!vapid) {
      warnVapidUnusable(null);
      logAttempt("push.test", { deviceId, outcome: "vapid_unusable" }, undefined, rows.length);
      return { ok: false, status: 503 };
    }
    const payload = encoder.encode(JSON.stringify(PushPayloadSchema.parse({ v: 1, t: "Monoptah", b: "Test notification", url: "/settings", tag: TEST_TAG })));
    report = await deliver(env, { rows, payload, urgency: "high", topic: await topicFor(TEST_TAG), vapid, honorBackoff: false }, deps, nowMs);
  }
  const counts = { sent: report?.sent ?? 0, gone: report?.gone ?? 0, failed: report?.failed ?? 0, backoff: report?.backoff ?? 0 };
  // The notifications are already out: a failed write is logged, not turned into a 500 that invites a repeat.
  let writeFailed = false;
  try {
    await env.DB.batch([
      ...(report?.statements ?? []),
      auditStatement(env, {
        action: "push.test",
        actorKind: "user",
        actorUserId: userId,
        targetType: "device",
        targetId: deviceId,
        meta: { deviceId, recipients: rows.length, ...counts },
      }),
    ]);
  } catch {
    writeFailed = true;
  }
  logAttempt("push.test", { deviceId, outcome: writeFailed ? "write_failed" : rows.length === 0 ? "no_recipients" : "sent" }, report, rows.length);
  return { ok: true, body: { sent: counts.sent, results: report?.results ?? [] } };
}

/** `POST /api/push/test` (session). 3 per user per 10 minutes, counted here because the user is known only after auth. */
export const pushTest: Handler<AppEnv> = async (c) => {
  const { user } = requireAuth(c);
  const parsed = PushTestBody.safeParse(await c.req.json().catch(() => null));
  if (!parsed.success) return apiError(c, 400, "invalid", "Invalid test request");
  // The user id is hashed (salted) so it never appears in `rate_limits`; the breach audit names the user as actor.
  const key = rateLimitKey("push-test", "user", await identifierHash(c.env, "user", user.id));
  const limit = await consume(c.env, key, LIMITS["push-test"].limit, LIMITS["push-test"].windowSeconds);
  if (!limit.ok) {
    if (limit.firstBreach) await auditBreach(c, "push-test", LIMITS["push-test"], user.id);
    c.header("Retry-After", String(limit.retryAfter));
    return apiError(c, 429, "rate_limited", "Too many requests");
  }
  const result = await runPushTest(c.env, user.id, parsed.data.deviceId);
  if (!result.ok) return result.status === 404 ? apiError(c, 404, "not_found", "Device not found") : apiError(c, 503, "internal", "Push is not configured");
  return respond(c, PushTestResponse, result.body);
};
