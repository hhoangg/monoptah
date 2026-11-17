import { noCleanup } from "../cleanup.ts";

/**
 * STUB (WS-R, Wave 1): delete `pending` `device_grants` older than 7 days.
 * The index `device_grants_state_updated_at_idx` already exists.
 */
export const cleanup = noCleanup;
