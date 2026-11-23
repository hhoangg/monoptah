import { useQueryClient } from "@tanstack/react-query";
import { useState } from "react";
import { Button } from "@/ui/button";
import { Notice } from "@/ui/Notice";
import { SETTINGS_ANCHOR, SettingsCard, SettingsCardsUnderHeading, SettingsRow } from "@/ui/SettingsCard";
import { NotificationsSection } from "./NotificationsSection";
import { PushOnboarding } from "./PushOnboarding";
import { PushTest } from "./PushTest";
import { useDesktops } from "@/features/desktops/api";
import { safeName } from "@/features/desktops/format";
import { loadPauseRecord } from "@/features/push/pauseRecord";
import { pauseAll, resumeAll, type BulkResult } from "./pushPrefs";
import { useAction } from "./useAction";
import { DelayedSpinner } from "@/ui/DelayedSpinner";

function plural(count: number): string {
  return `${count} ${count === 1 ? "desktop" : "desktops"}`;
}

function bulkMessage(enabled: boolean, { changed, skipped }: BulkResult): string {
  if (changed === 0) {
    return enabled
      ? "Nothing to resume: Pause all has not turned any desktop off from this browser."
      : "Nothing to pause: notifications are already off for your desktops, or their settings could not be read.";
  }
  const done = enabled ? `Notifications are back on for ${plural(changed)}.` : `Notifications are paused for ${plural(changed)}.`;
  return !enabled && skipped > 0 ? `${done} ${plural(skipped)} left as they are, because they were already off or their settings could not be read.` : done;
}

/**
 * Account level notifications (P-05): enable in this browser, send a test, Pause all and Resume all. `pausedCount` is the
 * number of desktops that Pause all turned off and Resume all can turn back on; the page owns it (see `NotificationsPage`)
 * because the per-desktop switches on the same page change it too.
 */
export function NotificationsCard({ pausedCount, onPausedChange }: { pausedCount: number; onPausedChange: () => void }) {
  const queryClient = useQueryClient();
  const bulk = useAction();
  // `paused` is the paused count right after the action: when the per-desktop switches change it later, the result is stale.
  const [shown, setLast] = useState<{ enabled: boolean; result: BulkResult; paused: number } | null>(null);
  const last = shown && shown.paused === pausedCount ? shown : null;

  async function apply(enabled: boolean) {
    await bulk.run(async () => {
      const result = enabled ? await resumeAll(queryClient) : await pauseAll(queryClient);
      setLast({ enabled, result, paused: Object.keys(loadPauseRecord()).length });
      onPausedChange();
    });
  }

  const partial = last && last.result.failed.length > 0;
  return (
    <SettingsCard
      title="Notifications"
      aria-label="Notifications"
      footnote="Pause all turns off the desktops that are on. Resume all turns back on only the ones it paused."
    >
      <PushOnboarding id="notification-permission" installTo="/settings/install" />
      <PushTest id="test-notification" />
      <SettingsRow
        id="pause-notifications"
        className={SETTINGS_ANCHOR}
        icon="notifications"
        title="Pause notifications"
        description="Turn notifications off for every desktop that has them on, and back on later with the same choices."
        trailing={
          <span className="flex gap-2">
            <Button type="button" variant="outline" size="sm" disabled={bulk.busy} aria-busy={bulk.busy} onClick={() => void apply(false)}>
              Pause all
            </Button>
            <Button type="button" variant="outline" size="sm" disabled={bulk.busy || pausedCount === 0} aria-busy={bulk.busy} onClick={() => void apply(true)}>
              Resume all
            </Button>
          </span>
        }
      />
      {last && !partial ? (
        <Notice variant="info" className="my-1.5">
          {bulkMessage(last.enabled, last.result)}
        </Notice>
      ) : null}
      {partial ? (
        <Notice variant="error" className="my-1.5">
          Could not change {last.result.failed.join(", ")}. The other desktops were changed. Try again.
        </Notice>
      ) : null}
      {bulk.error ? (
        <Notice variant="error" className="my-1.5">
          {bulk.error}
        </Notice>
      ) : null}
    </SettingsCard>
  );
}

/**
 * "Notifications per desktop": the three category switches of every desktop, inline (they used to be links to each
 * desktop's settings). Pause all and Resume all write the same query cache entries, so these switches follow them.
 */
export function NotificationsPerDesktop({ onSaved }: { onSaved?: () => void } = {}) {
  const desktops = useDesktops();
  const list = desktops.data?.desktops ?? [];
  return (
    <section id="notifications-per-desktop" aria-labelledby="notifications-per-desktop-title" className={`flex ${SETTINGS_ANCHOR} flex-col gap-6`}>
      <div className="flex flex-col gap-1 border-b border-stroke px-1 pb-2">
        <h2 id="notifications-per-desktop-title" className="text-h font-semibold text-content">
          Notifications per desktop
        </h2>
        <p className="text-small text-muted-text">Each desktop sends approvals and questions, finished agents and messages from Monos. Choose which ones for each desktop.</p>
      </div>
      {desktops.isPending ? <DelayedSpinner label="Loading desktops" className="px-1.5 py-2" /> : null}
      {desktops.data && list.length === 0 ? <p className="px-1 text-small text-muted-text">No desktops yet. Pair a desktop to choose its notifications.</p> : null}
      <SettingsCardsUnderHeading>
        {list.map((desktop) => (
          <NotificationsSection key={desktop.id} desktopId={desktop.id} title={safeName(desktop.name, "Unnamed desktop")} onSaved={onSaved} />
        ))}
      </SettingsCardsUnderHeading>
    </section>
  );
}
