import { useEffect, useState } from "react";
import { cn } from "@/lib/utils";
import { Icon } from "./Icon";

/** True once `active` has been true for `delayMs` (design-system.md 12.18: loading shows after 250 ms). */
export function useDelayed(active: boolean, delayMs = 250): boolean {
  const [elapsed, setElapsed] = useState(false);
  useEffect(() => {
    if (!active) return;
    const timer = window.setTimeout(() => setElapsed(true), delayMs);
    return () => {
      window.clearTimeout(timer);
      setElapsed(false);
    };
  }, [active, delayMs]);
  return active && elapsed;
}

export interface DelayedSpinnerProps {
  /** Spinner only appears after this many ms, so quick work never flashes. */
  delayMs?: number;
  /** Screen reader text. */
  label?: string;
  className?: string;
}

/** Muted spinner that mounts its icon after 250 ms. Renders nothing before that. Inside a Button it takes the label colour. */
export function DelayedSpinner({ delayMs = 250, label = "Loading", className }: DelayedSpinnerProps) {
  const visible = useDelayed(true, delayMs);
  if (!visible) return null;
  return (
    <span role="status" className={cn("inline-flex items-center text-muted-text", className)}>
      <Icon name="spinner" className="animate-spin" />
      <span className="sr-only">{label}</span>
    </span>
  );
}
