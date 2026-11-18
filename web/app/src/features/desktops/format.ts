// Display helpers of the desktop screens. Owner: WS-M (U-01). Times from the API are unix seconds.
import { formatRelative } from "@/features/settings/format";
import { safeLabel } from "@/features/accounts/pair/untrusted";
import type { Grant } from "./api";

export function presenceLabel(online: boolean): string {
  return online ? "Online" : "Offline";
}

/** "Online now", "Last seen 5 minutes ago" or "Never connected". */
export function lastSeenLabel(lastSeenAt: number | null, online: boolean, now?: number): string {
  if (online) return "Online now";
  if (lastSeenAt === null) return "Never connected";
  return `Last seen ${formatRelative(lastSeenAt, now)}`;
}

/** "macOS, v1.2.3", either part alone, or null when neither is known. Both come from the desktop: untrusted. */
export function versionLabel(os: string | null, appVersion: string | null): string | null {
  const parts = [os ? safeLabel(os, 32, "") : "", appVersion ? `v${safeLabel(appVersion.replace(/^v/i, ""), 24, "")}` : ""].filter((part) => part !== "" && part !== "v");
  return parts.length > 0 ? parts.join(", ") : null;
}

export function permissionLabel(permission: Grant["permission"]): string {
  if (permission === "control") return "Control";
  if (permission === "read") return "Read only";
  return "Not set yet";
}

export function grantStateLabel(state: Grant["state"]): string {
  if (state === "confirmed") return "Confirmed";
  if (state === "pending") return "Pending confirmation";
  return "Revoked";
}

/** A device or desktop name chosen by someone else: plain text with hidden characters removed. */
export function safeName(name: string, fallback = "Unnamed"): string {
  return safeLabel(name, 64, fallback);
}
