import { useQuery, useQueryClient } from "@tanstack/react-query";
import { useState, type ReactNode } from "react";
import { PUSH_CATEGORIES } from "@monoptah/protocol";
import { forgetPaused } from "@/features/push/pauseRecord";
import { savePushPrefs, type PushPrefs } from "@/features/accounts/queries";
import { DelayedSpinner } from "@/ui/DelayedSpinner";
import { Notice } from "@/ui/Notice";
import { SettingsCard, SettingsRow } from "@/ui/SettingsCard";
import { Switch } from "@/ui/switch";
import { settingsErrorMessage } from "./copy";
import { pushPrefsKey, pushPrefsQuery } from "./pushPrefs";

type Category = (typeof PUSH_CATEGORIES)[number];

export const CATEGORY_LABELS: Record<Category, { title: string; description: string }> = {
  agentInput: { title: "Approvals and questions", description: "When an agent needs your answer" },
  agentFinished: { title: "Agent finished", description: "When an agent completes its work" },
  monoMessages: { title: "Messages from Monos", description: "When a Mono writes to you" },
};

/** What the switches show while the saved prefs load (the same defaults the Worker applies); they are disabled until then. */
export const DRAFT_PREFS: PushPrefs = { enabled: true, categories: { agentFinished: true, agentInput: true, monoMessages: true } };

const DEFAULT_FOOTNOTE = "Saved to your account for this desktop. Permission to show notifications in this browser is asked separately.";

/**
 * Notification categories of one desktop (N-15). The prefs load with `GET /api/push/prefs/:desktopId`; a desktop
 * that never saved any shows the defaults (all on), which are what the Worker applies too. Each switch saves at once.
 * `title` heads the card (the notifications page passes the desktop name, one block per desktop); the switch labels then
 * name the desktop too, so several blocks on one page stay distinguishable to a screen reader.
 */
export function NotificationsSection({
  desktopId,
  title = "Notifications",
  footnote = DEFAULT_FOOTNOTE,
  onSaved,
}: {
  desktopId: string;
  title?: string;
  footnote?: ReactNode;
  /** Called after a choice was saved (and Pause all's record of this desktop dropped), so a page can re-read that record. */
  onSaved?: () => void;
}) {
  const queryClient = useQueryClient();
  const loaded = useQuery<PushPrefs>({ ...pushPrefsQuery(desktopId), staleTime: 0 });
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const prefs = loaded.data;
  // Until the answer arrives the switches show the defaults, disabled.
  const shown = prefs ?? DRAFT_PREFS;

  async function save(next: PushPrefs): Promise<void> {
    if (saving) return;
    setSaving(true);
    setError(null);
    try {
      const result = await savePushPrefs(desktopId, next);
      queryClient.setQueryData(pushPrefsKey(desktopId), result);
      // An explicit choice here replaces what Pause all recorded, so Resume all must not overwrite it.
      forgetPaused(desktopId);
      onSaved?.();
    } catch (caught) {
      setError(settingsErrorMessage(caught));
    } finally {
      setSaving(false);
    }
  }

  const named = title === "Notifications" ? (text: string) => text : (text: string) => `${text} (${title})`;
  return (
    <SettingsCard title={title} footnote={footnote}>
      {loaded.isError ? (
        <Notice variant="error" className="my-1.5">
          {settingsErrorMessage(loaded.error)}
        </Notice>
      ) : null}
      {prefs === undefined && !loaded.isError ? <DelayedSpinner label="Loading notification settings" className="px-1.5 py-2" /> : null}
      <SettingsRow
        icon="notifications"
        title="Notifications from this desktop"
        trailing={<Switch checked={shown.enabled} disabled={saving || prefs === undefined} aria-label={named("Notifications from this desktop")} onCheckedChange={(enabled) => void save({ ...shown, enabled })} />}
      />
      {PUSH_CATEGORIES.map((category) => (
        <SettingsRow
          key={category}
          title={CATEGORY_LABELS[category].title}
          description={CATEGORY_LABELS[category].description}
          className="pl-[4.5rem] before:left-[4.5rem]"
          trailing={
            <Switch
              checked={shown.categories[category] ?? true}
              disabled={saving || prefs === undefined || !shown.enabled}
              aria-label={named(CATEGORY_LABELS[category].title)}
              onCheckedChange={(on) => void save({ ...shown, categories: { ...shown.categories, [category]: on } })}
            />
          }
        />
      ))}
      {error ? (
        <Notice variant="error" className="my-1.5">
          {error}
        </Notice>
      ) : null}
    </SettingsCard>
  );
}
