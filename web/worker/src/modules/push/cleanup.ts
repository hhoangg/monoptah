import { noCleanup } from "../cleanup.ts";

/**
 * No daily cleanup for push: a subscription is deleted inline when the push service answers 404 or 410 or after 5
 * consecutive failures (notify.ts), and on device forget and admin block. A pending or revoked grant stops
 * delivery at once (a confirmed grant is required). Sign-out does not delete subscriptions: the web client
 * calls `DELETE /api/push/subscriptions/:id` first (P-05, WS-S); deleting them on logout is a pending WS-A
 * change. A time based sweep would need an index on `last_success_at` and is not worth the write budget.
 */
export const cleanup = noCleanup;
