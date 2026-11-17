// State machine of "Enable notifications" (P-05, N-14). Permission is requested only from `enable()`, which a
// click handler calls: nothing here prompts on mount, on render or on focus.
import { useCallback, useEffect, useState } from "react";
import { isApiError } from "@/features/accounts/api";
import { getSavedPushSubscription, isPushSupported, PushUnavailableError, subscribeToPush, unsubscribeFromPush } from "@/features/push";
import { ensureDevice } from "@/link";
import { settingsErrorMessage } from "./copy";
import { useStandalone } from "./standalone";

export type PushOnboardingState =
  | "checking"
  /** Not running as an installed app: no button, a note that leads to /install. */
  | "not-installed"
  /** Installed, but this browser has no Push API. */
  | "unsupported"
  /** Installed and supported: the Enable button is shown (permission default, or granted without a subscription). */
  | "ready"
  | "denied"
  | "on";

// VAPID rotation (security.md push, step 4): once per page load, an existing subscription is re-sent. `subscribeToPush`
// compares the key with the Worker's current one, replaces the browser subscription when it differs, and upserts by endpoint.
let rotationChecked = false;

export const SIGN_IN_FIRST = "Sign in first, then turn on notifications.";

function hasPushManager(): boolean {
  return typeof PushManager !== "undefined";
}

function describe(error: unknown): string {
  if (isApiError(error, "network")) return settingsErrorMessage(error);
  if (error instanceof PushUnavailableError) return error.reason === "permission" ? "Notifications are not allowed for Monoptah in this browser." : "This browser cannot receive push notifications.";
  if (typeof error === "object" && error !== null && "status" in error && error.status === 401) return SIGN_IN_FIRST;
  return settingsErrorMessage(error);
}

export function usePushOnboarding() {
  const standalone = useStandalone();
  const [subscribed, setSubscribed] = useState<boolean | null>(null);
  const [permission, setPermission] = useState<NotificationPermission | null>(() => (typeof Notification === "undefined" ? null : Notification.permission));
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [notice, setNotice] = useState<string | null>(null);

  useEffect(() => {
    let live = true;
    const refresh = (): void => {
      void getSavedPushSubscription()
        .catch(() => null)
        .then((saved) => {
          if (!live) return;
          setPermission(typeof Notification === "undefined" ? null : Notification.permission);
          setSubscribed(saved !== null);
          if (saved && !rotationChecked && typeof Notification !== "undefined" && Notification.permission === "granted") {
            rotationChecked = true;
            void subscribeToPush(saved.deviceId).catch(() => undefined);
          }
        });
    };
    refresh();
    // The user can change the permission in the browser or OS settings while the app is in the background.
    const onVisible = () => {
      if (document.visibilityState === "visible") refresh();
    };
    document.addEventListener("visibilitychange", onVisible);
    return () => {
      live = false;
      document.removeEventListener("visibilitychange", onVisible);
    };
  }, []);

  let state: PushOnboardingState;
  if (!standalone) state = "not-installed";
  else if (!hasPushManager() || !isPushSupported() || permission === null) state = "unsupported";
  else if (permission === "denied") state = "denied";
  else if (subscribed === null) state = "checking";
  else if (permission === "granted" && subscribed) state = "on";
  else state = "ready";

  const enable = useCallback(async () => {
    if (busy) return;
    setBusy(true);
    setError(null);
    setNotice(null);
    try {
      const result = await Notification.requestPermission();
      setPermission(result);
      if (result === "default") {
        setNotice("No permission was given. Press the button again when you want notifications.");
        return;
      }
      if (result === "granted") {
        const { deviceId } = await ensureDevice();
        await subscribeToPush(deviceId);
        setSubscribed(true);
      }
    } catch (caught) {
      setError(describe(caught));
    } finally {
      setBusy(false);
    }
  }, [busy]);

  const disable = useCallback(async () => {
    if (busy) return;
    setBusy(true);
    setError(null);
    setNotice(null);
    try {
      await unsubscribeFromPush();
      setSubscribed(false);
    } catch (caught) {
      setError(describe(caught));
    } finally {
      setBusy(false);
    }
  }, [busy]);

  return { state, busy, error, notice, enable, disable };
}
