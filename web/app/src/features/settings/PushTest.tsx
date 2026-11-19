import { useEffect, useState } from "react";
import { isApiError } from "@/features/accounts/api";
import { sendPushTest, type PushTestResult } from "@/features/accounts/queries";
import { getSavedPushSubscription, hasBrowserSubscription } from "@/features/push";
import { PUSH_CHANGED_EVENT, PUSH_RESUBSCRIBED_EVENT } from "@/features/push/subscribe";
import { Button } from "@/ui/button";
import { Notice } from "@/ui/Notice";
import { SettingsRow } from "@/ui/SettingsCard";
import { settingsErrorMessage } from "./copy";

export { PUSH_CHANGED_EVENT };

export const TEST_SENT = "Test notification sent. It should appear in a few seconds.";
export const TEST_NO_SUBSCRIPTION = "This device has no push subscription. Turn notifications on first.";
export const TEST_RATE_LIMITED = "Too many tests. Try again in a few minutes.";
export const TEST_NOT_CONFIGURED = "Push is not configured on this server.";
export const TEST_EXPIRED = "This subscription expired; turn notifications off and on again.";

export interface TestOutcome {
  kind: "success" | "failure";
  message: string;
}

/** The name of the push service behind a subscription endpoint, for the failure text. */
export function pushServiceName(endpoint: string | null): string {
  try {
    const host = new URL(endpoint ?? "").hostname;
    if (host.endsWith("push.apple.com")) return "Apple's push service";
    if (host.endsWith("googleapis.com")) return "Google's push service";
    if (host.endsWith("mozilla.com") || host.endsWith("mozaws.net")) return "Mozilla's push service";
  } catch {
    // An unknown endpoint gets the generic name.
  }
  return "The push service";
}

/** What the `POST /api/push/test` answer means for the user. `service` names the push service ("Apple's push service"). */
export function describeTestResult(result: PushTestResult, service = "The push service"): TestOutcome {
  if (result.sent > 0) return { kind: "success", message: TEST_SENT };
  const failed = result.results.find((item) => item.outcome !== "ok");
  if (!failed) return { kind: "failure", message: TEST_NO_SUBSCRIPTION };
  switch (failed.outcome) {
    case "gone":
      return { kind: "failure", message: TEST_EXPIRED };
    case "skipped_key":
      return { kind: "failure", message: "This subscription was made with an old key; turn notifications off and on again." };
    case "backoff":
      return { kind: "failure", message: `${service} asked to wait before the next push. Try again in a few minutes.` };
    default:
      return {
        kind: "failure",
        message: failed.status === 0 ? `${service} did not answer. Try again.` : `${service} refused it (HTTP ${failed.status}).`,
      };
  }
}

function describeTestError(error: unknown): string {
  const status = isApiError(error) ? error.status : undefined;
  if (isApiError(error, "rate_limited") || status === 429) return TEST_RATE_LIMITED;
  if (status === 503) return TEST_NOT_CONFIGURED;
  if (status === 404) return "This device is not known to the server. Turn notifications off and on again.";
  return settingsErrorMessage(error);
}

interface Ready {
  deviceId: string | null;
  endpoint: string | null;
}

async function readReady(): Promise<Ready> {
  const none: Ready = { deviceId: null, endpoint: null };
  if (typeof Notification === "undefined" || Notification.permission !== "granted") return none;
  const saved = await getSavedPushSubscription().catch(() => null);
  if (!saved || !(await hasBrowserSubscription())) return none;
  let endpoint: string | null = null;
  try {
    const registration = await navigator.serviceWorker.getRegistration();
    endpoint = (await registration?.pushManager.getSubscription())?.endpoint ?? null;
  } catch {
    // The endpoint only names the push service in a failure text.
  }
  return { deviceId: saved.deviceId, endpoint };
}

/** "Send test notification" of this device (Settings > Notifications): a real push through the Worker, with the result inline. */
export function PushTest() {
  const [ready, setReady] = useState<Ready>({ deviceId: null, endpoint: null });
  const [sending, setSending] = useState(false);
  const [outcome, setOutcome] = useState<TestOutcome | null>(null);

  useEffect(() => {
    let live = true;
    const refresh = () => {
      void readReady().then((next) => {
        if (live) setReady(next);
      });
    };
    refresh();
    const onVisible = () => {
      if (document.visibilityState === "visible") refresh();
    };
    document.addEventListener("visibilitychange", onVisible);
    window.addEventListener(PUSH_CHANGED_EVENT, refresh);
    window.addEventListener(PUSH_RESUBSCRIBED_EVENT, refresh);
    return () => {
      live = false;
      document.removeEventListener("visibilitychange", onVisible);
      window.removeEventListener(PUSH_CHANGED_EVENT, refresh);
      window.removeEventListener(PUSH_RESUBSCRIBED_EVENT, refresh);
    };
  }, []);

  async function send(): Promise<void> {
    if (sending || ready.deviceId === null) return;
    setSending(true);
    setOutcome(null);
    try {
      setOutcome(describeTestResult(await sendPushTest(ready.deviceId), pushServiceName(ready.endpoint)));
    } catch (caught) {
      setOutcome({ kind: "failure", message: describeTestError(caught) });
    } finally {
      setSending(false);
    }
  }

  return (
    <>
      <SettingsRow
        icon="notifications"
        title="Test delivery"
        description={ready.deviceId === null ? "Turn notifications on for this device first." : "Sends a real push to this device to check that it arrives."}
        trailing={
          <Button type="button" variant="outline" size="sm" disabled={sending || ready.deviceId === null} aria-busy={sending} onClick={() => void send()}>
            {sending ? "Sending..." : "Send test notification"}
          </Button>
        }
      />
      <div role="status" aria-live="polite">
        {outcome ? (
          <Notice variant={outcome.kind === "success" ? "success" : "error"} className="my-1.5">
            {outcome.message}
          </Notice>
        ) : null}
      </div>
    </>
  );
}
