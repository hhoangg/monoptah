// Presence decision of a desktop socket (architecture.md 4.4 "Presence"), pure so it can be unit tested.
import { PRESENCE_TIMEOUT_MS } from "@monoptah/protocol";

/**
 * True when the desktop socket counts as online at `now` (ms): the last ping (auto-response timestamp) or, before
 * the first ping, the join time, is at most `PRESENCE_TIMEOUT_MS` old.
 */
export function isDesktopOnline(now: number, joinedAt: number, lastPing: number | null): boolean {
  return now - (lastPing ?? joinedAt) <= PRESENCE_TIMEOUT_MS;
}
