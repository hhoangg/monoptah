import { ErrorComponent, useRouter, type ErrorComponentProps } from "@tanstack/react-router";
import { useEffect } from "react";
import { rateLimitWaitMs, isRateLimited } from "@/lib/rateLimit";
import { DelayedSpinner } from "./DelayedSpinner";

/**
 * Default error component of the router. A 429 that still reaches it (the retries ran out) is not a crash: the
 * "Too many requests" notice is already showing, so this waits for `Retry-After` and loads the route again.
 * Every other error keeps the router's own error screen.
 */
export function RouteError(props: ErrorComponentProps) {
  const router = useRouter();
  const { error } = props;
  const limited = isRateLimited(error);

  useEffect(() => {
    if (!isRateLimited(error)) return;
    const timer = setTimeout(() => void router.invalidate(), rateLimitWaitMs(error, 0));
    return () => clearTimeout(timer);
  }, [error, router]);

  if (!limited) return <ErrorComponent {...props} />;
  return <DelayedSpinner label="Retrying" className="m-6" />;
}
