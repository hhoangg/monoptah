import { useQueryClient } from "@tanstack/react-query";
import { useState } from "react";
import { Button } from "@/ui/button";
import { Notice } from "@/ui/Notice";
import { SettingsCard, SettingsRow } from "@/ui/SettingsCard";
import { PushOnboarding, CATEGORY_EXPLAINERS } from "./PushOnboarding";
import { setEnabledEverywhere, type BulkResult } from "./pushPrefs";
import { useAction } from "./useAction";

/** Account level notifications (P-05): enable in this browser, what the three categories mean, Pause all. */
export function NotificationsCard() {
  const queryClient = useQueryClient();
  const bulk = useAction();
  const [last, setLast] = useState<{ enabled: boolean; result: BulkResult } | null>(null);

  async function apply(enabled: boolean) {
    await bulk.run(async () => {
      const result = await setEnabledEverywhere(queryClient, enabled);
      setLast({ enabled, result });
    });
  }

  const partial = last && last.result.failed.length > 0;
  return (
    <SettingsCard
      title="Notifications"
      aria-label="Notifications"
      footnote="Choose which categories each desktop sends in that desktop's settings. Pause all changes every desktop at once."
    >
      <PushOnboarding />
      <ul className="m-0 flex list-none flex-col p-0" aria-label="Notification categories">
        {CATEGORY_EXPLAINERS.map((category) => (
          <SettingsRow key={category.title} as="li" title={category.title} description={category.description} />
        ))}
      </ul>
      <SettingsRow
        icon="notifications"
        title="Pause all"
        description="Turn notifications off for every desktop, or back on. Category choices are kept for desktops you changed in this session; others come back with every category on."
        trailing={
          <span className="flex gap-2">
            <Button type="button" variant="outline" size="sm" disabled={bulk.busy} aria-busy={bulk.busy} onClick={() => void apply(false)}>
              Pause all
            </Button>
            <Button type="button" variant="outline" size="sm" disabled={bulk.busy} aria-busy={bulk.busy} onClick={() => void apply(true)}>
              Resume all
            </Button>
          </span>
        }
      />
      {last && !partial ? (
        <Notice variant="info" className="my-1.5">
          {last.result.total === 0
            ? "No desktops yet, so there was nothing to change."
            : last.enabled
              ? `Notifications are on for ${last.result.total} ${last.result.total === 1 ? "desktop" : "desktops"}.`
              : `Notifications are paused for ${last.result.total} ${last.result.total === 1 ? "desktop" : "desktops"}.`}
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
