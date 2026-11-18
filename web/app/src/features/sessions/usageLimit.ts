// Copied from src/features/sessions/model/usageLimit.ts at ca73ac2b
// `formatUsageLimitReset` and the validator only; `formatResetDuration` is carried over from
// src/features/providers/model/rateLimits.ts at ca73ac2b. The desktop owns the reset windows and the
// recovery rules (it sends them in `usageLimit`); the web never infers them. The validator is the
// phone/web-side check of that state before a resume is sent.
import type { UsageLimit } from "@monoptah/protocol";

/**
 * Compact remaining duration, flooring to whole units: "47m", "3h 54m", "6d 7h".
 * Returns "now" once the window has already reset.
 */
export function formatResetDuration(ms: number): string {
  if (ms <= 0) return "now";
  const totalMins = Math.floor(ms / 60_000);
  if (totalMins < 60) return `${totalMins}m`;
  const hours = Math.floor(totalMins / 60);
  const mins = totalMins % 60;
  if (hours >= 24) {
    const days = Math.floor(hours / 24);
    const remHours = hours % 24;
    return remHours > 0 ? `${days}d ${remHours}h` : `${days}d`;
  }
  return mins > 0 ? `${hours}h ${mins}m` : `${hours}h`;
}

/** "3:16 AM · in 4h 42m" today, "Sep 26, 3:16 AM · in 1d 4h" later. */
export function formatUsageLimitReset(resetsAt: number, now: number): string {
  const reset = new Date(resetsAt);
  const sameDay = reset.toDateString() === new Date(now).toDateString();
  const when = reset.toLocaleString(undefined, {
    ...(sameDay ? {} : { month: "short", day: "numeric" }),
    hour: "numeric",
    minute: "2-digit",
  });
  return `${when} · in ${formatResetDuration(resetsAt - now)}`;
}

/** The reset is still ahead of `now`. */
export function usageLimitWaiting(limit: Pick<UsageLimit, "resetsAt">, now: number): boolean {
  return limit.resetsAt != null && limit.resetsAt > now;
}

/**
 * Checks the usage-limit state the desktop sent before the web acts on it (resume now, or arm
 * resume at reset). `state` is the latest `usageLimit` of the watch frame, `token` is the one
 * the user saw. Throws an Error with user-facing text, like the desktop's validator.
 */
export function validateUsageLimit(
  state: UsageLimit | null | undefined,
  token: unknown,
  automatic: boolean,
  now = Date.now(),
): UsageLimit {
  if (!state) throw new Error("This session is no longer paused by a usage limit");
  if (state.token !== token) throw new Error("This session changed. Wait for it to refresh and try again.");
  if (!(automatic ? state.canAutoResume : state.canResume)) {
    throw new Error(
      state.resetsAt != null && state.resetsAt > now && !automatic
        ? `${state.resetLabel}. Enable Resume at reset or try again after the allowance resets.`
        : "This session cannot continue right now. Check it on desktop and try again.",
    );
  }
  return state;
}
