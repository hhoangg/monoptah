// The accent picked for one Mono's chat (F-78 "chat colour", ported from mobile monoAccent.ts). Phone-local by
// design: kept in this browser's localStorage per desktop and Mono (`monoptah.monoAccents`), never sent to the desktop.
import { useSyncExternalStore } from "react";
import { useAppearance } from "@/lib/appearance";
import { ACCENT_COLORS } from "@/features/themes/swatches";
import { ACCENTS, type Accent } from "@/styles/themes";

export const MONO_ACCENTS_KEY = "monoptah.monoAccents";

type Store = Readonly<Record<string, Accent>>;

function load(): Store {
  try {
    const parsed: unknown = JSON.parse(window.localStorage.getItem(MONO_ACCENTS_KEY) ?? "{}");
    if (!parsed || typeof parsed !== "object" || Array.isArray(parsed)) return {};
    return Object.fromEntries(
      Object.entries(parsed).filter((entry): entry is [string, Accent] => typeof entry[1] === "string" && (ACCENTS as readonly string[]).includes(entry[1])),
    );
  } catch {
    return {};
  }
}

let accents: Store = load();
const listeners = new Set<() => void>();
const subscribe = (listener: () => void) => {
  listeners.add(listener);
  return () => {
    listeners.delete(listener);
  };
};

const keyOf = (desktopId: string, monoId: string) => `${desktopId}:${monoId}`;

/** Draws this Mono's chat in `accent`, or back in the app's own for "theme". */
export function setMonoAccent(desktopId: string, monoId: string, accent: Accent): void {
  const { [keyOf(desktopId, monoId)]: _removed, ...rest } = accents;
  accents = accent === "theme" ? rest : { ...rest, [keyOf(desktopId, monoId)]: accent };
  try {
    window.localStorage.setItem(MONO_ACCENTS_KEY, JSON.stringify(accents));
  } catch {
    // storage blocked or full: the pick lasts until the tab closes
  }
  listeners.forEach((listener) => listener());
}

/** The accent picked for this Mono's chat ("theme" when none). */
export function useMonoAccent(desktopId: string, monoId: string): Accent {
  return useSyncExternalStore(subscribe, () => accents[keyOf(desktopId, monoId)] ?? "theme");
}

/** The CSS variable override that draws a subtree in the Mono's accent; undefined for "theme". */
export function useMonoAccentStyle(desktopId: string, monoId: string): Record<string, string> | undefined {
  const accent = useMonoAccent(desktopId, monoId);
  const scheme = useAppearance((state) => state.scheme);
  if (accent === "theme") return undefined;
  return { "--accent-color": ACCENT_COLORS[accent][scheme === "dark" ? 0 : 1] };
}

/** Re-reads the stored values (for tests). */
export function reloadMonoAccents(): void {
  accents = load();
  listeners.forEach((listener) => listener());
}
