// Closing delivered notifications from the page (P-04, F-83): opening or showing a session clears the
// notices of that session, so the lock screen does not keep what the user has just read.
import { useEffect } from "react";

/** Closes every delivered notification with this tag (the tag is the session id). Returns how many. */
export async function closeSessionNotifications(tag: string): Promise<number> {
  if (typeof navigator === "undefined" || !("serviceWorker" in navigator)) return 0;
  try {
    const registration = await navigator.serviceWorker.getRegistration();
    if (!registration) return 0;
    const notifications = await registration.getNotifications({ tag });
    for (const notification of notifications) notification.close();
    return notifications.length;
  } catch {
    // Notifications API unavailable (some in-app browsers): nothing to close.
    return 0;
  }
}

/**
 * For the session screen (WS-T): closes the notices of `sessionId` when the screen opens or the session
 * changes, and again whenever the tab becomes visible while it is showing that session.
 */
export function useCloseNotificationsOnOpen(sessionId: string | undefined): void {
  useEffect(() => {
    if (!sessionId) return;
    void closeSessionNotifications(sessionId);
    const onVisible = () => {
      if (document.visibilityState === "visible") void closeSessionNotifications(sessionId);
    };
    document.addEventListener("visibilitychange", onVisible);
    return () => document.removeEventListener("visibilitychange", onVisible);
  }, [sessionId]);
}
