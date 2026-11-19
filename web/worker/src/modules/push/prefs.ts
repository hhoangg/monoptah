// Push preferences as the Worker applies them. A missing `push_prefs` row means the defaults (enabled, every
// category on), the same state the web Settings screen shows before anything is saved. The opt-in is the
// subscription itself (a user gesture plus the browser permission) and the desktop-side device selection.
import { PUSH_CATEGORIES } from "@monoptah/protocol";

export type PushPrefsMap = Record<(typeof PUSH_CATEGORIES)[number], boolean>;

export interface PrefsState {
  enabled: boolean;
  categories: PushPrefsMap;
  /** False when no row exists and the defaults are returned. */
  stored: boolean;
}

/** The columns of a `push_prefs` row as read (both null when the LEFT JOIN found no row). */
export interface PrefsColumns {
  enabled: number | null;
  categories: string | null;
}

function parseCategories(raw: string | null): Record<string, unknown> {
  try {
    const value: unknown = JSON.parse(raw ?? "{}");
    return typeof value === "object" && value !== null && !Array.isArray(value) ? (value as Record<string, unknown>) : {};
  } catch {
    return {};
  }
}

/** No row: the defaults. A row: its `enabled`, and each category on unless the stored JSON says `false` (a missing key is on).
 *
 * Decision: a category absent from a stored row is on, so a category added later is on even for users who turned
 * the others off. The opt-in is the subscription itself, and the stored map only records what the user switched off. */
export function resolvePrefs(row: PrefsColumns): PrefsState {
  const stored = row.enabled !== null;
  const map = stored ? parseCategories(row.categories) : {};
  const categories = Object.fromEntries(PUSH_CATEGORIES.map((key) => [key, map[key] !== false])) as PushPrefsMap;
  return { enabled: stored ? row.enabled === 1 : true, categories, stored };
}
