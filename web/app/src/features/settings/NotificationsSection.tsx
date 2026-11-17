import { useQuery, useQueryClient } from "@tanstack/react-query";
import { useState } from "react";
import { PUSH_CATEGORIES } from "@monoptah/protocol";
import { savePushPrefs, type PushPrefs } from "@/features/accounts/queries";
import { Button } from "@/ui/button";
import { Notice } from "@/ui/Notice";
import { SettingsCard, SettingsRow } from "@/ui/SettingsCard";
import { Switch } from "@/ui/switch";
import { settingsErrorMessage } from "./copy";

type Category = (typeof PUSH_CATEGORIES)[number];

export const CATEGORY_LABELS: Record<Category, { title: string; description: string }> = {
  agentInput: { title: "Approvals and questions", description: "When an agent needs your answer" },
  agentFinished: { title: "Agent finished", description: "When an agent completes its work" },
  monoMessages: { title: "Messages from Monos", description: "When a Mono writes to you" },
};

/** The starting point of the draft shown before anything is known. It is never sent without an explicit Save. */
export const DRAFT_PREFS: PushPrefs = { enabled: true, categories: { agentFinished: true, agentInput: true, monoMessages: true } };

export const pushPrefsKey = (desktopId: string) => ["settings", "push-prefs", desktopId] as const;

/**
 * Notification categories of one desktop (N-15). The contract has no GET for prefs (a gap reported
 * to WS-K/WS-A), so this browser cannot read what is stored. Until a save returns, the switches are
 * a draft with an explicit "Save these settings" button and a note; nothing invented is ever sent
 * as a side effect of flipping one switch. After a save the PUT result is kept in the query cache
 * and each switch saves at once, as before.
 */
export function NotificationsSection({ desktopId }: { desktopId: string }) {
  const queryClient = useQueryClient();
  const savedQuery = useQuery<PushPrefs | null>({
    queryKey: pushPrefsKey(desktopId),
    queryFn: () => queryClient.getQueryData<PushPrefs>(pushPrefsKey(desktopId)) ?? null,
    initialData: null,
    staleTime: Infinity,
    gcTime: Infinity,
  });
  const saved = savedQuery.data;
  const [draft, setDraft] = useState<PushPrefs>(DRAFT_PREFS);
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const prefs = saved ?? draft;
  const unknown = saved === null;

  async function save(next: PushPrefs): Promise<void> {
    if (saving) return;
    setSaving(true);
    setError(null);
    try {
      const result = await savePushPrefs(desktopId, next);
      queryClient.setQueryData(pushPrefsKey(desktopId), result);
    } catch (caught) {
      setError(settingsErrorMessage(caught));
    } finally {
      setSaving(false);
    }
  }

  /** Unknown: only edit the draft. Known: save the change at once. */
  function change(next: PushPrefs): void {
    if (unknown) setDraft(next);
    else void save(next);
  }

  return (
    <SettingsCard title="Notifications" footnote="Saved to your account for this desktop. Permission to show notifications in this browser is asked separately.">
      {unknown ? (
        <Notice variant="info" title="Current settings not loaded" className="my-1.5">
          This browser cannot read the settings saved for this desktop yet. The switches below are not your saved values. Set them the way you want, then press Save: that saves all three categories and the main switch as shown.
        </Notice>
      ) : null}
      <SettingsRow
        icon="notifications"
        title="Notifications from this desktop"
        trailing={<Switch checked={prefs.enabled} disabled={saving} aria-label="Notifications from this desktop" onCheckedChange={(enabled) => change({ ...prefs, enabled })} />}
      />
      {PUSH_CATEGORIES.map((category) => (
        <SettingsRow
          key={category}
          title={CATEGORY_LABELS[category].title}
          description={CATEGORY_LABELS[category].description}
          className="pl-12"
          trailing={
            <Switch
              checked={prefs.categories[category] ?? true}
              disabled={saving || !prefs.enabled}
              aria-label={CATEGORY_LABELS[category].title}
              onCheckedChange={(on) => change({ ...prefs, categories: { ...prefs.categories, [category]: on } })}
            />
          }
        />
      ))}
      {unknown ? (
        <div className="px-1.5 py-2">
          <Button type="button" className="w-full" disabled={saving} aria-busy={saving} onClick={() => void save(draft)}>
            Save these settings
          </Button>
        </div>
      ) : null}
      {error ? (
        <Notice variant="error" className="my-1.5">
          {error}
        </Notice>
      ) : null}
    </SettingsCard>
  );
}
