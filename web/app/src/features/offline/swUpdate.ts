// Update flow of the service worker (N-22, architecture.md 3.5): when a new version is waiting, a toast
// "A new version of Monoptah is ready" offers Reload. Nothing reloads by itself, and a tap while a send
// is in progress waits until the send has ended (the page reload would drop the message).
import { toast } from "@/ui/sonner";

export const UPDATE_TOAST_ID = "sw-update";
export const UPDATE_TOAST_TITLE = "A new version of Monoptah is ready";
export const UPDATE_WAITING_COPY = "Reloading as soon as the message is sent.";
/** How often a pending reload looks again at the send check. */
export const SEND_POLL_MS = 500;

type SendCheck = () => boolean;

let sendCheck: SendCheck = () => false;

/**
 * WS-T registers a function that returns true while a message is being sent (a turn is being submitted
 * and not yet acknowledged). The default says no send is ever in progress. Returns the previous check.
 */
export function setSendInProgressCheck(check: SendCheck): SendCheck {
  const previous = sendCheck;
  sendCheck = check;
  return previous;
}

export function isSendInProgress(): boolean {
  try {
    return sendCheck();
  } catch {
    // A broken check must not block an update for ever, nor start one in the middle of a send: say no send.
    return false;
  }
}

/** Runs `apply` now, or once `isSendInProgress()` turns false. Returns a function that cancels a pending run. */
export function applyWhenNoSend(apply: () => void, pollMs: number = SEND_POLL_MS): () => void {
  if (!isSendInProgress()) {
    apply();
    return () => {};
  }
  const timer = setInterval(() => {
    if (isSendInProgress()) return;
    clearInterval(timer);
    apply();
  }, pollMs);
  return () => clearInterval(timer);
}

/** Shows the persistent update toast. `onReload` applies the update (skip waiting, then reload). */
export function showUpdateToast(onReload: () => void): void {
  toast(UPDATE_TOAST_TITLE, {
    id: UPDATE_TOAST_ID,
    duration: Infinity,
    action: {
      label: "Reload",
      onClick: (event) => {
        if (!isSendInProgress()) {
          onReload();
          return;
        }
        // Keep the toast open (sonner closes it on an action unless the click is prevented) and say why.
        event.preventDefault();
        toast(UPDATE_TOAST_TITLE, { id: UPDATE_TOAST_ID, description: UPDATE_WAITING_COPY, duration: Infinity, dismissible: false });
        applyWhenNoSend(onReload);
      },
    },
  });
}
