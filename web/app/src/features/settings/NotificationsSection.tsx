import { useQuery, useQueryClient } from "@tanstack/react-query";
import { useState } from "react";
import { PUSH_CATEGORIES } from "@monoptah/protocol";
import { savePushPrefs, type PushPrefs } from "@/features/accounts/queries";
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

/** The contract has no GET for prefs, so a desktop starts with everything on until a save returns. */
export const DEFAULT_PREFS: PushPrefs = { enabled: true, categories: { agentFinished: true, agentInput: true, monoMessages: true } };

export const pushPrefsKey = (desktopId: string) => ["settings", "push-prefs", desktopId] as const;

/** Notification categories of one desktop (N-15). Saved with PUT /api/push/prefs/:desktopId; the last saved result is kept in the query cache only. */
export function NotificationsSection({ desktopId }: { desktopId: string }) {
  const queryClient = useQueryClient();
  const prefsQuery = useQuery({
    queryKey: pushPrefsKey(desktopId),
    queryFn: () => queryClient.getQueryData<PushPrefs>(pushPrefsKey(desktopId)) ?? DEFAULT_PREFS,
    initialData: DEFAULT_PREFS,
    staleTime: Infinity,
    gcTime: Infinity,
  });
  const prefs = prefsQuery.data;
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState<string | null>(null);

  async function save(next: PushPrefs): Promise<void> {
    if (saving) return;
    setSaving(true);
    setError(null);
    try {
      const saved = await savePushPrefs(desktopId, next);
      queryClient.setQueryData(pushPrefsKey(desktopId), saved);
    } catch (caught) {
      setError(settingsErrorMessage(caught));
    } finally {
      setSaving(false);
    }
  }

  return (
    <SettingsCard title="Notifications" footnote="Saved to your account for this desktop. Permission to show notifications in this browser is asked separately.">
      <SettingsRow
        icon="notifications"
        title="Notifications from this desktop"
        trailing={<Switch checked={prefs.enabled} disabled={saving} aria-label="Notifications from this desktop" onCheckedChange={(enabled) => void save({ ...prefs, enabled })} />}
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
              onCheckedChange={(on) => void save({ ...prefs, categories: { ...prefs.categories, [category]: on } })}
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
