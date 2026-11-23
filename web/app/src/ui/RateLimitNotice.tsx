import { useEffect, useState } from "react";
import { clearRateLimited, rateLimitSecondsLeft, useRateLimitState } from "@/lib/rateLimitState";
import { Notice } from "./Notice";

/**
 * Small non-blocking "Too many requests" status (the Worker answered 429). Mount it once, outside the router outlet, so
 * it shows even while the root `beforeLoad` is pending. It counts down to the end of the wait, then clears the state.
 */
export function RateLimitNotice() {
  const until = useRateLimitState((state) => state.until);
  const since = useRateLimitState((state) => state.since);
  const [tickedAt, setTickedAt] = useState(() => Date.now());

  useEffect(() => {
    if (until === null) return;
    const timer = setInterval(() => {
      const current = Date.now();
      setTickedAt(current);
      if (current >= until) clearRateLimited();
    }, 1000);
    return () => clearInterval(timer);
  }, [until]);

  // A tick can be older than the 429 that just arrived; `since` is never older, so the first paint is right.
  const seconds = rateLimitSecondsLeft(until, Math.max(tickedAt, since));
  if (seconds === 0) return null;
  return (
    <div className="pointer-events-none fixed inset-x-0 top-[max(0.75rem,env(safe-area-inset-top))] z-50 flex justify-center px-3" data-testid="rate-limit-notice">
      <Notice className="pointer-events-auto w-full max-w-sm bg-surface">
        {/* The live region text never changes, so a screen reader announces it once, not on every tick of the countdown. */}
        Too many requests. Retrying shortly.
        <span aria-hidden="true" data-testid="rate-limit-seconds">{` (${seconds} s)`}</span>
      </Notice>
    </div>
  );
}
