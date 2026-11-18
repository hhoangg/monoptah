// Labels and the read-only note come from the shared vocabulary (`../model`, `../copy`), so the session
// screens and the instance list cannot drift apart.
export { KIND_LABELS, STATE_LABELS, kindLabel } from "../model";
export { READ_ONLY_NOTE } from "../copy";

export const STOP_REPORT_LABELS = {
  stopping: "Stopping",
  takingLong: "Stopping is taking longer than usual",
  stillRunning: "The instance is still running",
} as const;

export const INSTANCE_BUSY_MESSAGE = "The desktop is archiving or converting a session on this instance. Try again in a moment.";

export const costLabel = (perHour: number): string => `about $${perHour.toFixed(2)}/hour (estimate)`;

export const otherLabel = (count: number): string => `${count} other instance${count === 1 ? "" : "s"} on this desktop`;

/** "Last used: 5 minutes ago". `value` is unix time in seconds or milliseconds. */
export function lastUsedLabel(value: number | null | undefined, nowMs: number): string {
  if (value === null || value === undefined) return "Last used: unknown";
  const ms = value < 1e11 ? value * 1000 : value;
  const minutes = Math.floor(Math.max(0, nowMs - ms) / 60_000);
  const plural = (n: number, unit: string) => `${n} ${unit}${n === 1 ? "" : "s"} ago`;
  if (minutes < 1) return "Last used: just now";
  if (minutes < 60) return `Last used: ${plural(minutes, "minute")}`;
  const hours = Math.floor(minutes / 60);
  if (hours < 24) return `Last used: ${plural(hours, "hour")}`;
  return `Last used: ${plural(Math.floor(hours / 24), "day")}`;
}
