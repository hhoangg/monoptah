// Time text of the terminal: the limits the desktop enforces, shown as hints, and the countdown of the code.
import { useEffect, useState } from "react";

/** "45 s", "12 min", "7 h 52 min" (rounded down to the minute; under a minute it counts seconds). */
export function durationText(ms: number): string {
  const total = Math.max(0, Math.floor(ms / 1000));
  if (total < 60) return `${total} s`;
  const minutes = Math.floor(total / 60);
  if (minutes < 60) return `${minutes} min`;
  const hours = Math.floor(minutes / 60);
  const rest = minutes % 60;
  return rest === 0 ? `${hours} h` : `${hours} h ${rest} min`;
}

/** "0:45": the code countdown. */
export function clockText(ms: number): string {
  const total = Math.max(0, Math.ceil(ms / 1000));
  return `${Math.floor(total / 60)}:${String(total % 60).padStart(2, "0")}`;
}

export interface LimitsInput {
  openedAt: number;
  idleTimeoutMs: number;
  maxDurationMs: number;
}

/** "Closes after 15 min without typing, and in 7 h 52 min at the latest." */
export function limitsHint(limits: LimitsInput, now: number): string {
  const parts: string[] = [];
  if (limits.idleTimeoutMs > 0) parts.push(`closes after ${durationText(limits.idleTimeoutMs)} without typing`);
  if (limits.maxDurationMs > 0) parts.push(`${parts.length > 0 ? "and " : "closes "}in ${durationText(limits.openedAt + limits.maxDurationMs - now)} at the latest`);
  const text = parts.join(", ");
  return text === "" ? "" : `${text.charAt(0).toUpperCase()}${text.slice(1)}.`;
}

/** The current time, refreshed every `everyMs` while mounted (for countdowns and hints). */
export function useNow(everyMs: number, active = true): number {
  const [now, setNow] = useState(() => Date.now());
  useEffect(() => {
    if (!active) return;
    const timer = window.setInterval(() => setNow(Date.now()), everyMs);
    return () => window.clearInterval(timer);
  }, [everyMs, active]);
  return now;
}
