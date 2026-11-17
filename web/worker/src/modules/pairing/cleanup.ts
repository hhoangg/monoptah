import { noCleanup } from "../cleanup.ts";

/**
 * STUB (WS-R, R-03, Wave 1): delete `pairing_requests` 24 hours after they resolved or expired.
 * The index `pairing_requests_expires_at_idx` already exists.
 */
export const cleanup = noCleanup;
