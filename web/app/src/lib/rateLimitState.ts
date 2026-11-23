// The "slow down" state of the Worker's per-session limit (120 calls per 60 s). `apiFetch` sets it when any call
// answers 429 and clears it on the next success; the notice (ui/RateLimitNotice.tsx) shows it and clears it when
// the window passes. It is a plain store so it can be read outside React (the router error component, tests).
import { create } from "zustand";

/** Used when a 429 carries no `Retry-After`: the Worker always sends one, so this is only a proxy's answer. */
export const FALLBACK_RETRY_AFTER_SECONDS = 5;

interface RateLimitState {
  /** Epoch ms until which the server asked us to wait, or null when calls are not limited. */
  until: number | null;
  /** Epoch ms of the 429 that set `until`: a clock reading no older than the wait itself. */
  since: number;
}

export const useRateLimitState = create<RateLimitState>(() => ({ until: null, since: 0 }));

/** A call answered 429: the wait lasts until now + `retryAfterSeconds`. A later answer never shortens an earlier one. */
export function noteRateLimited(retryAfterSeconds: number | null, now: number = Date.now()): void {
  const until = now + (retryAfterSeconds ?? FALLBACK_RETRY_AFTER_SECONDS) * 1000;
  useRateLimitState.setState((state) => ({ until: state.until === null || until > state.until ? until : state.until, since: now }));
}

export function clearRateLimited(): void {
  if (useRateLimitState.getState().until !== null) useRateLimitState.setState({ until: null, since: 0 });
}

/** Whole seconds left in the wait (at least 1 while it lasts), or 0 when calls are not limited. */
export function rateLimitSecondsLeft(until: number | null, now: number = Date.now()): number {
  if (until === null || now >= until) return 0;
  return Math.max(1, Math.ceil((until - now) / 1000));
}
