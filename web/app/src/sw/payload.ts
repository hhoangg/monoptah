// Push payload handling for the service worker (architecture.md 8). `PushPayloadSchema` in
// @monoptah/protocol is the contract, but importing zod would add tens of KiB to the worker, so this is
// a hand-written validator of the same shape; payload.test.ts checks that both agree on shared fixtures.
import { FALLBACK_URL, safeNotificationUrl } from "./safeUrl";

export interface ParsedPushPayload {
  v: 1;
  t: string;
  b: string;
  url: string;
  tag: string;
}

/** Same rules as `PushPayloadSchema`: an object with v === 1 and string `t`, `b`, `url`, `tag`. */
export function parsePushPayload(raw: unknown): ParsedPushPayload | null {
  if (typeof raw !== "object" || raw === null || Array.isArray(raw)) return null;
  const value = raw as Record<string, unknown>;
  if (value.v !== 1) return null;
  const { t, b, url, tag } = value;
  if (typeof t !== "string" || typeof b !== "string" || typeof url !== "string" || typeof tag !== "string") return null;
  return { v: 1, t, b, url, tag };
}

export const GENERIC_TITLE = "Monoptah";
/** Same-origin precached images: the app icon and a monochrome badge for the Android status bar. */
export const NOTIFICATION_ICON = "/icons/icon-192.png";
export const NOTIFICATION_BADGE = "/icons/badge-96.png";
export const GENERIC_BODY = "Open Monoptah to see what happened.";
/** The `data` of every notification: only the click target. */
export interface NotificationData {
  url: string;
}

export interface NotificationSpec {
  title: string;
  options: NotificationOptions & { data: NotificationData };
}

/** The notification for a payload. Text comes from `t` and `b` verbatim; nothing else is shown. */
export function notificationFor(payload: ParsedPushPayload | null): NotificationSpec {
  if (!payload) return { title: GENERIC_TITLE, options: { body: GENERIC_BODY, icon: NOTIFICATION_ICON, badge: NOTIFICATION_BADGE, data: { url: FALLBACK_URL } } };
  // An unusable URL still shows the notice, and a click opens the root.
  const url = safeNotificationUrl(payload.url) ?? FALLBACK_URL;
  const options: NotificationSpec["options"] = { body: payload.b, icon: NOTIFICATION_ICON, badge: NOTIFICATION_BADGE, data: { url } };
  // One notification per session: a newer one replaces the older (the Worker also sends a Topic header).
  if (payload.tag !== "") options.tag = payload.tag;
  return { title: payload.t, options };
}

/** Reads the push data as JSON; anything unreadable is null (the generic notification). */
export function readPushData(data: { json(): unknown } | null): unknown {
  if (!data) return null;
  try {
    return data.json();
  } catch {
    return null;
  }
}
