// State machine of "Enable notifications" (P-05, N-14). Permission is requested only from `enable()`, which a
// click handler calls: nothing here prompts on mount, on render or on focus.
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { useCallback, useEffect, useState } from "react";
import { isApiError } from "@/features/accounts/api";
import { meQuery } from "@/features/accounts/queries";
import { getSavedPushSubscription, hasBrowserSubscription, isPushSupported, PushUnavailableError, subscribeToPush, unsubscribeFromPush } from "@/features/push";
import { ensureDevice } from "@/link";
import { settingsErrorMessage } from "./copy";
import { useStandalone } from "./standalone";

export type PushOnboardingState =
  | "checking"
  /** Not running as an installed app: no button, a note that leads to /install. */
  | "not-installed"
  /** Installed, but this browser has no Push API. */
  | "unsupported"
  /** Installed and supported, but nobody is signed in (on public /install): a sign-in link instead of a button that would waste the OS prompt. */
  | "signed-out"
  /** Installed and supported: the Enable button is shown (permission default, or granted without a subscription). */
  | "ready"
  | "denied"
  | "on";

// VAPID rotation (security.md push, step 4): once per page load, an existing subscription is re-sent. `subscribeToPush`
// compares the key with the Worker's current one, replaces the browser subscription when it differs, and upserts by endpoint.
let rotationChecked = false;

/** Test helper: allow the once-per-load rotation check to run again. */
export function resetPushRotationCheck(): void {
  rotationChecked = false;
}

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
  const queryClient = useQueryClient();
  // Only for an installed app: it decides between the Enable button and a sign-in link without a wasted OS prompt.
  const me = useQuery({ ...meQuery, enabled: standalone, retry: false });
  const signedOut = me.isError && isApiError(me.error) && me.error.status === 401;
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
        .then(async (saved) => {
          if (!live) return;
          setPermission(typeof Notification === "undefined" ? null : Notification.permission);
          // A subscription saved for another account is not this account's "on" (it is removed when `me` loads).
          const userId = queryClient.getQueryData(meQuery.queryKey)?.user.id ?? null;
          const owned = saved !== null && (userId === null || saved.userId === userId);
          setSubscribed(owned);
          // Rotation re-send: only for a signed-in user, for the subscription that user made, with the browser
          // subscription still there. Never silently subscribe again after a removal or a sign-out.
          if (saved && owned && userId !== null && !rotationChecked && typeof Notification !== "undefined" && Notification.permission === "granted") {
            rotationChecked = true;
            if (await hasBrowserSubscription()) void subscribeToPush(saved.deviceId, userId).catch(() => undefined);
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
  }, [queryClient]);

  let state: PushOnboardingState;
  if (!standalone) state = "not-installed";
  else if (!hasPushManager() || !isPushSupported() || permission === null) state = "unsupported";
  else if (permission === "denied") state = "denied";
  else if (subscribed === null) state = "checking";
  else if (permission === "granted" && subscribed) state = "on";
  else if (signedOut) state = "signed-out";
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
        const userId = queryClient.getQueryData(meQuery.queryKey)?.user.id;
        if (!userId) throw Object.assign(new Error("Signed out"), { status: 401 });
        const { deviceId } = await ensureDevice();
        await subscribeToPush(deviceId, userId);
        setSubscribed(true);
      }
    } catch (caught) {
      setError(describe(caught));
    } finally {
      setBusy(false);
    }
  }, [busy, queryClient]);

  const disable = useCallback(async () => {
    if (busy) return;
    setBusy(true);
    setError(null);
    setNotice(null);
    try {
      await unsubscribeFromPush();
      setSubscribed(false);
    } catch (caught) {
      // The browser side and the saved state are gone even when the Worker call failed: this browser is off.
      setSubscribed(false);
      setError(describe(caught));
    } finally {
      setBusy(false);
    }
  }, [busy]);

  return { state, busy, error, notice, enable, disable };
}
