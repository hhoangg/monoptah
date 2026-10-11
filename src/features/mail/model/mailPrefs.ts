import { useSyncExternalStore } from "react";

/**
 * Mail prefs in localStorage. The `monocode.` prefix is deliberately kept
 * (renaming it would drop every saved preference); `monocode.mail.enabled`
 * lives with the other feature toggles in settings.ts.
 */

export const MAIL_PREF_KEYS = {
  selectedMailbox: "monocode.mail.selectedMailbox",
  selectedFolder: "monocode.mail.selectedFolder",
  collapsedDomains: "monocode.mail.collapsedDomains",
  syncIntervalSeconds: "monocode.mail.syncIntervalSeconds",
  listWidth: "monocode.mail.listWidth",
  readingPaneSide: "monocode.mail.readingPaneSide",
  density: "monocode.mail.density",
  showImages: "monocode.mail.showImages",
} as const;

export type ReadingPaneSide = "right" | "bottom" | "off";
export type MailDensity = "comfortable" | "compact";
export type ShowImages = "ask" | "always" | "never";

export const SYNC_INTERVAL_DEFAULT_SECONDS = 3600;
/** Rust clamps to the same floor; mirrored so the UI never offers less. */
export const SYNC_INTERVAL_MIN_SECONDS = 60;
export const LIST_WIDTH_DEFAULT = 340;

const eventFor = (name: string) => `monocode:mail-${name}-change`;

function readRaw(key: string): string | null {
  try {
    return localStorage.getItem(key);
  } catch {
    return null;
  }
}

function writeRaw(key: string, value: string | null) {
  try {
    if (value === null) localStorage.removeItem(key);
    else localStorage.setItem(key, value);
  } catch {
    // private mode / quota
  }
}

function prefStore<T>(
  name: string,
  key: string,
  fallback: T,
  parse: (raw: string) => T | undefined,
  serialize: (value: T) => string | null,
) {
  const event = eventFor(name);
  // Holds a value only while storage failed to keep it, so the control still
  // moves for this window when localStorage is unavailable.
  let unsaved: { value: T } | null = null;
  // useSyncExternalStore needs the same reference until the data changes, which
  // matters for the array pref.
  let cachedRaw: string | null | undefined;
  let cachedValue: T = fallback;

  const load = (): T => {
    if (unsaved) return unsaved.value;
    const raw = readRaw(key);
    if (raw !== cachedRaw) {
      cachedRaw = raw;
      cachedValue = (raw === null ? undefined : parse(raw)) ?? fallback;
    }
    return cachedValue;
  };
  const save = (value: T) => {
    const raw = serialize(value);
    writeRaw(key, raw);
    unsaved = readRaw(key) === raw ? null : { value };
    if (typeof window === "undefined") return;
    window.dispatchEvent(new CustomEvent<T>(event, { detail: value }));
  };
  const subscribe = (onStoreChange: () => void) => {
    if (typeof window === "undefined") return () => {};
    // Other windows write the same localStorage; `storage` reports their saves.
    const onStorage = (storage: StorageEvent) => {
      if (storage.key !== key && storage.key !== null) return;
      unsaved = null;
      onStoreChange();
    };
    window.addEventListener(event, onStoreChange);
    window.addEventListener("storage", onStorage);
    return () => {
      window.removeEventListener(event, onStoreChange);
      window.removeEventListener("storage", onStorage);
    };
  };
  const use = () => useSyncExternalStore(subscribe, load, () => fallback);
  return { load, save, subscribe, use, event };
}

function enumPref<T extends string>(
  name: string,
  key: string,
  values: readonly T[],
  fallback: T,
) {
  return prefStore<T>(
    name,
    key,
    fallback,
    (raw) => values.find((value) => value === raw),
    (value) => value,
  );
}

const nullableString = (name: string, key: string) =>
  prefStore<string | null>(
    name,
    key,
    null,
    (raw) => (raw === "" ? undefined : raw),
    (value) => (value === "" ? null : value),
  );

const positiveInt = (name: string, key: string, fallback: number, min = 1) =>
  prefStore<number>(
    name,
    key,
    fallback,
    (raw) => {
      const n = Number(raw);
      return Number.isInteger(n) && n >= min ? n : undefined;
    },
    (value) => String(Math.max(min, Math.round(value))),
  );

export const selectedMailbox = nullableString(
  "selectedmailbox",
  MAIL_PREF_KEYS.selectedMailbox,
);
export const selectedFolder = nullableString(
  "selectedfolder",
  MAIL_PREF_KEYS.selectedFolder,
);

const NO_DOMAINS: string[] = [];
export const collapsedDomains = prefStore<string[]>(
  "collapseddomains",
  MAIL_PREF_KEYS.collapsedDomains,
  NO_DOMAINS,
  (raw) => {
    try {
      const parsed: unknown = JSON.parse(raw);
      return Array.isArray(parsed) &&
        parsed.every((item) => typeof item === "string")
        ? (parsed as string[])
        : undefined;
    } catch {
      return undefined;
    }
  },
  (value) => JSON.stringify(value),
);

/** App-wide. Must be pushed to Rust with `mailSyncIntervalSet` on every change and at startup. */
export const syncIntervalSeconds = positiveInt(
  "syncintervalseconds",
  MAIL_PREF_KEYS.syncIntervalSeconds,
  SYNC_INTERVAL_DEFAULT_SECONDS,
  SYNC_INTERVAL_MIN_SECONDS,
);
export const listWidth = positiveInt(
  "listwidth",
  MAIL_PREF_KEYS.listWidth,
  LIST_WIDTH_DEFAULT,
);
export const readingPaneSide = enumPref<ReadingPaneSide>(
  "readingpaneside",
  MAIL_PREF_KEYS.readingPaneSide,
  ["right", "bottom", "off"],
  "right",
);
export const density = enumPref<MailDensity>(
  "density",
  MAIL_PREF_KEYS.density,
  ["comfortable", "compact"],
  "comfortable",
);
/** Mirrors the per-source value in the database; this is the default for new views. */
export const showImages = enumPref<ShowImages>(
  "showimages",
  MAIL_PREF_KEYS.showImages,
  ["ask", "always", "never"],
  "ask",
);

export const useSelectedMailbox = selectedMailbox.use;
export const useSelectedFolder = selectedFolder.use;
export const useCollapsedDomains = collapsedDomains.use;
export const useSyncIntervalSeconds = syncIntervalSeconds.use;
export const useListWidth = listWidth.use;
export const useReadingPaneSide = readingPaneSide.use;
export const useDensity = density.use;
export const useShowImages = showImages.use;
