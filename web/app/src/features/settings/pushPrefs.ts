// Account-level "Pause all" (N-15). The Worker has no global pause field: prefs are one row per user and
// desktop (`PUT /api/push/prefs/:desktopId`), and there is no GET. So Pause all sets `enabled: false` on
// every desktop and Resume all sets `enabled: true`, both keeping the categories this session last knew.
// What this session has not seen (no GET) falls back to every category on, and the UI says so.
import { PUSH_CATEGORIES } from "@monoptah/protocol";
import type { QueryClient } from "@tanstack/react-query";
import { desktopsQuery, savePushPrefs, type PushPrefs } from "@/features/accounts/queries";

export const DEFAULT_PREFS: PushPrefs = { enabled: true, categories: Object.fromEntries(PUSH_CATEGORIES.map((key) => [key, true])) as PushPrefs["categories"] };

export const pushPrefsKey = (desktopId: string) => ["settings", "push-prefs", desktopId] as const;

export interface BulkResult {
  total: number;
  failed: string[];
}

/** Sets `enabled` on every desktop of the account. Successful PUT results go to the query cache. */
export async function setEnabledEverywhere(queryClient: QueryClient, enabled: boolean): Promise<BulkResult> {
  const { desktops } = await queryClient.fetchQuery({ ...desktopsQuery, staleTime: 0 });
  const outcomes = await Promise.allSettled(
    desktops.map(async (desktop) => {
      const known = queryClient.getQueryData<PushPrefs | null>(pushPrefsKey(desktop.id)) ?? DEFAULT_PREFS;
      const saved = await savePushPrefs(desktop.id, { enabled, categories: known.categories });
      queryClient.setQueryData(pushPrefsKey(desktop.id), saved);
    }),
  );
  const failed = desktops.filter((_, index) => outcomes[index]?.status === "rejected").map((desktop) => desktop.name);
  return { total: desktops.length, failed };
}
