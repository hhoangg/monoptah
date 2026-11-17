// Account-level "Pause all" and "Resume all" (N-15). The Worker keeps push prefs per user and desktop
// (`PUT /api/push/prefs/:desktopId`, push_prefs primary key (user_id, desktop_id)) and has no GET and no
// global pause field, so this browser can only know a desktop's prefs when it saved them or saw the PUT result
// in this page load. Other devices of the same account can change them too, so nothing is guessed:
//  - Pause all turns off only the desktops whose prefs are known and on, keeps their categories, and records
//    them (features/push/pauseRecord.ts) so that
//  - Resume all turns back on exactly those desktops with the categories they had, and never a desktop that
//    was off or unknown. Desktops with unknown prefs are left alone and counted.
import { PUSH_CATEGORIES } from "@monoptah/protocol";
import type { QueryClient } from "@tanstack/react-query";
import { desktopsQuery, savePushPrefs, type PushPrefs } from "@/features/accounts/queries";
import { loadPauseRecord, savePauseRecord } from "@/features/push/pauseRecord";

export const DEFAULT_PREFS: PushPrefs = { enabled: true, categories: Object.fromEntries(PUSH_CATEGORIES.map((key) => [key, true])) as PushPrefs["categories"] };

export const pushPrefsKey = (desktopId: string) => ["settings", "push-prefs", desktopId] as const;

export interface BulkResult {
  /** Desktops this call turned off or on. */
  changed: number;
  /** Desktops left alone because their saved prefs are unknown here (Pause all) or were not paused by Pause all (Resume all). */
  skipped: number;
  /** Names of desktops whose PUT failed. */
  failed: string[];
}

export async function pauseAll(queryClient: QueryClient): Promise<BulkResult> {
  const { desktops } = await queryClient.fetchQuery({ ...desktopsQuery, staleTime: 0 });
  const record = loadPauseRecord();
  const targets = desktops.filter((desktop) => queryClient.getQueryData<PushPrefs | null>(pushPrefsKey(desktop.id))?.enabled === true);
  const outcomes = await Promise.allSettled(
    targets.map(async (desktop) => {
      const known = queryClient.getQueryData<PushPrefs>(pushPrefsKey(desktop.id)) ?? DEFAULT_PREFS;
      const saved = await savePushPrefs(desktop.id, { enabled: false, categories: known.categories });
      queryClient.setQueryData(pushPrefsKey(desktop.id), saved);
      record[desktop.id] = { ...known.categories };
    }),
  );
  savePauseRecord(record);
  const failed = targets.filter((_, index) => outcomes[index]?.status === "rejected").map((desktop) => desktop.name);
  return { changed: targets.length - failed.length, skipped: desktops.length - targets.length, failed };
}

export async function resumeAll(queryClient: QueryClient): Promise<BulkResult> {
  const { desktops } = await queryClient.fetchQuery({ ...desktopsQuery, staleTime: 0 });
  const record = loadPauseRecord();
  const targets = desktops.filter((desktop) => record[desktop.id] !== undefined);
  const outcomes = await Promise.allSettled(
    targets.map(async (desktop) => {
      const categories = record[desktop.id];
      if (!categories) return;
      const saved = await savePushPrefs(desktop.id, { enabled: true, categories });
      queryClient.setQueryData(pushPrefsKey(desktop.id), saved);
      delete record[desktop.id];
    }),
  );
  // Records of desktops that no longer exist are dropped with the rest.
  for (const id of Object.keys(record)) if (!desktops.some((desktop) => desktop.id === id)) delete record[id];
  savePauseRecord(record);
  const failed = targets.filter((_, index) => outcomes[index]?.status === "rejected").map((desktop) => desktop.name);
  return { changed: targets.length - failed.length, skipped: desktops.length - targets.length, failed };
}
