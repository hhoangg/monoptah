// "3 outstanding per IP" for `pair/start` (security.md 3.5): a pairing request is outstanding while it is
// pending and unexpired. Counted from `pairing_requests.ip_hash`; the WS-R pair/start handler calls it.
import { apiError, type AppContext } from "../../http.ts";
import { clientIp, ipHash } from "../../lib/ip.ts";
import { nowSeconds } from "../../lib/time.ts";

export const MAX_OUTSTANDING_PAIRINGS = 3;
/** `Retry-After` when the cap is reached: the pairing requests live a few minutes, so ask to retry in a minute. */
const RETRY_AFTER_SECONDS = 60;

/** Statement of the count, exported for the query plan test (index `pairing_requests_ip_status_expires_idx`). */
export const OUTSTANDING_PAIRINGS_SQL = "SELECT COUNT(*) AS n FROM pairing_requests WHERE ip_hash = ? AND status = 'pending' AND expires_at > ?";

/**
 * Answers 429 with `Retry-After` when the caller's IP already has 3 or more outstanding pairing requests, or
 * null when another one may be started. Read only: the handler inserts the row (with the same `ip_hash`).
 */
export async function limitOutstandingPairings(c: AppContext, now: number = nowSeconds()): Promise<Response | null> {
  const hash = await ipHash(c.env, clientIp(c.req.raw));
  const row = await c.env.DB.prepare(OUTSTANDING_PAIRINGS_SQL).bind(hash, now).first<{ n: number }>();
  if ((row?.n ?? 0) < MAX_OUTSTANDING_PAIRINGS) return null;
  c.header("Retry-After", String(RETRY_AFTER_SECONDS));
  return apiError(c, 429, "rate_limited", "Too many pending pairing requests");
}
