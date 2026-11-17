// What "Pause all" changed, so "Resume all" can put exactly that back (N-15). The Worker keeps push prefs per
// user and desktop and has no GET, so the browser cannot know a desktop's categories unless this tab saw them.
// The record holds only the desktops that Pause all turned off and the categories they had at that moment:
// ids and booleans, nothing sensitive. It is dropped when the session ends or another account signs in.
import { PUSH_CATEGORIES } from "@monoptah/protocol";

export const PAUSE_RECORD_KEY = "monoptah.pushPaused";

export type PausedCategories = Record<(typeof PUSH_CATEGORIES)[number], boolean>;
export type PauseRecord = Record<string, PausedCategories>;

function parseCategories(value: unknown): PausedCategories | null {
  if (typeof value !== "object" || value === null) return null;
  const record = value as Record<string, unknown>;
  const result: Partial<PausedCategories> = {};
  for (const key of PUSH_CATEGORIES) {
    if (typeof record[key] !== "boolean") return null;
    result[key] = record[key];
  }
  return result as PausedCategories;
}

export function loadPauseRecord(): PauseRecord {
  try {
    const raw: unknown = JSON.parse(localStorage.getItem(PAUSE_RECORD_KEY) ?? "null");
    if (typeof raw !== "object" || raw === null || Array.isArray(raw)) return {};
    const result: PauseRecord = {};
    for (const [desktopId, value] of Object.entries(raw)) {
      const categories = parseCategories(value);
      if (categories) result[desktopId] = categories;
    }
    return result;
  } catch {
    return {};
  }
}

export function savePauseRecord(record: PauseRecord): void {
  try {
    if (Object.keys(record).length === 0) localStorage.removeItem(PAUSE_RECORD_KEY);
    else localStorage.setItem(PAUSE_RECORD_KEY, JSON.stringify(record));
  } catch {
    // Storage full or blocked: Resume all then has nothing to restore, which is the safe side.
  }
}

export function clearPauseRecord(): void {
  savePauseRecord({});
}

export function forgetPaused(desktopId: string): void {
  const record = loadPauseRecord();
  if (!(desktopId in record)) return;
  delete record[desktopId];
  savePauseRecord(record);
}
