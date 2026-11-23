import { Link, type LinkProps } from "@tanstack/react-router";
import { Button } from "@/ui/button";
import { DelayedSpinner } from "@/ui/DelayedSpinner";
import { Icon } from "@/ui/Icon";
import { Notice } from "@/ui/Notice";
import { cn } from "@/lib/utils";
import { SETTINGS_ANCHOR, SettingsRow } from "@/ui/SettingsCard";
import { CATEGORY_LABELS } from "./NotificationsSection";
import { detectPlatform } from "./standalone";
import { usePushOnboarding } from "./usePushOnboarding";

/** How to turn notifications back on after "Block". No deep links: a web page cannot open system settings. */
export function deniedHelp(platform = detectPlatform()): string {
  if (platform === "ios") return "Open the Settings app, choose Notifications, find Monoptah and turn on Allow Notifications. Then come back here.";
  if (platform === "android") return "Open Android Settings, choose Apps, then Monoptah, then Notifications, and turn them on. Then come back here.";
  return "Open your browser or system settings, find the site permissions or app notifications for Monoptah and set Notifications to Allow. Then come back here.";
}

export const CATEGORY_EXPLAINERS = (["agentInput", "agentFinished", "monoMessages"] as const).map((key) => CATEGORY_LABELS[key]);

/**
 * The enable/disable control of push (P-05), shared by /settings and /install. `Notification.requestPermission()`
 * runs only from the button click inside `usePushOnboarding().enable`. `installTo` is where "Install Monoptah" leads: the public
 * /install page by default, /settings/install inside the settings shell.
 */
export function PushOnboarding({ id, installTo = "/install" }: { id?: string; installTo?: LinkProps["to"] } = {}) {
  const push = usePushOnboarding();
  const { state } = push;
  return (
    <div id={id} className={cn("flex flex-col gap-2", SETTINGS_ANCHOR)}>
      {state === "checking" ? <DelayedSpinner label="Checking notification status" className="px-1.5" /> : null}
      {state === "not-installed" ? (
        <SettingsRow
          icon="download"
          title="Install Monoptah to get notifications"
          description="Notifications work from the installed app. On iPhone and iPad this is required."
          to={installTo}
        />
      ) : null}
      {state === "unsupported" ? (
        <Notice variant="info" title="Notifications are not available here">
          This browser cannot receive push notifications. Use a recent Chrome, Edge, Firefox or Safari 16.4 or newer.
        </Notice>
      ) : null}
      {state === "signed-out" ? (
        <SettingsRow icon="notifications" title="Sign in to turn on notifications" description="Notifications belong to your account." to="/login" />
      ) : null}
      {state === "ready" ? (
        <div className="px-1.5">
          <Button type="button" className="w-full sm:w-auto" disabled={push.busy} aria-busy={push.busy} onClick={() => void push.enable()}>
            <Icon name="notifications" size={18} />
            Enable notifications
          </Button>
        </div>
      ) : null}
      {state === "denied" ? (
        <Notice variant="info" title="Notifications are blocked">
          {deniedHelp()}
        </Notice>
      ) : null}
      {state === "on" ? (
        <SettingsRow
          icon="notifications"
          title="Notifications on"
          description="This browser receives the notifications you choose for each desktop."
          trailing={
            <Button type="button" variant="outline" size="sm" disabled={push.busy} aria-busy={push.busy} onClick={() => void push.disable()}>
              Turn off
            </Button>
          }
        />
      ) : null}
      {push.notice ? <Notice variant="info">{push.notice}</Notice> : null}
      {push.error ? (
        <Notice variant="error">
          {push.error}{" "}
          {push.error.startsWith("Sign in first") ? (
            <Link to="/login" className="underline">
              Sign in
            </Link>
          ) : null}
        </Notice>
      ) : null}
    </div>
  );
}
