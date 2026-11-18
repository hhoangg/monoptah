import { useQueryClient } from "@tanstack/react-query";
import { useState } from "react";
import { Button } from "@/ui/button";
import { Notice } from "@/ui/Notice";
import { SettingsCard, SettingsRow } from "@/ui/SettingsCard";
import { PushOnboarding } from "./PushOnboarding";
import { useDesktops } from "@/features/desktops/api";
import { safeName } from "@/features/desktops/format";
import { loadPauseRecord } from "@/features/push/pauseRecord";
import { pauseAll, resumeAll, type BulkResult } from "./pushPrefs";
import { useAction } from "./useAction";

function plural(count: number): string {
  return `${count} ${count === 1 ? "desktop" : "desktops"}`;
}

function bulkMessage(enabled: boolean, { changed, skipped }: BulkResult): string {
  if (changed === 0) {
    return enabled
      ? "Nothing to resume: Pause all has not turned any desktop off from this browser."
      : "Nothing was paused. This browser cannot read the saved settings of your desktops until you save them in each desktop's notification settings.";
  }
  const done = enabled ? `Notifications are back on for ${plural(changed)}.` : `Notifications are paused for ${plural(changed)}.`;
  return !enabled && skipped > 0 ? `${done} ${plural(skipped)} left as they are, because this browser cannot read their settings.` : done;
}

/** Account level notifications (P-05): enable in this browser, what the three categories mean, Pause all. */
export function NotificationsCard() {
  const queryClient = useQueryClient();
  const bulk = useAction();
  const desktops = useDesktops();
  const [last, setLast] = useState<{ enabled: boolean; result: BulkResult } | null>(null);

  // Desktops that Pause all turned off and Resume all can turn back on (read again after every action).
  const [pausedCount, setPausedCount] = useState(() => Object.keys(loadPauseRecord()).length);

  async function apply(enabled: boolean) {
    await bulk.run(async () => {
      const result = enabled ? await resumeAll(queryClient) : await pauseAll(queryClient);
      setLast({ enabled, result });
      setPausedCount(Object.keys(loadPauseRecord()).length);
    });
  }

  const partial = last && last.result.failed.length > 0;
  return (
    <SettingsCard
      title="Notifications"
      aria-label="Notifications"
      footnote="Pause all and Resume all only touch desktops whose settings this browser knows."
    >
      <PushOnboarding />
      <p className="px-1.5 py-1.5 text-small text-muted-text">
        Each desktop sends approvals and questions, finished agents and messages from Monos. Choose which ones in that desktop's settings.
      </p>
      {desktops.data && desktops.data.desktops.length > 0 ? (
        <ul className="m-0 flex list-none flex-col p-0" aria-label="Notification settings per desktop">
          {desktops.data.desktops.map((desktop) => (
            <li key={desktop.id} className="contents">
              <SettingsRow icon="computer" title={safeName(desktop.name, "Unnamed desktop")} description="Choose notification categories" to="/d/$desktopId/settings" params={{ desktopId: desktop.id }} />
            </li>
          ))}
        </ul>
      ) : null}
      <SettingsRow
        icon="notifications"
        title="Pause notifications"
        description="Turn notifications off for the desktops whose settings this browser knows, and back on later with the same choices. Other desktops are left as they are."
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
