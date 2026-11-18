// Device-local Mono preferences (F-13): "Monos only" (`monoptah.monosOnly`) and the Monos hidden from the
// home list, per desktop (`monoptah.hiddenMonos`). Never sent to the desktop.
import { create } from "zustand";

export const MONOS_ONLY_KEY = "monoptah.monosOnly";
export const HIDDEN_MONOS_KEY = "monoptah.hiddenMonos";

type Hidden = Readonly<Record<string, readonly string[]>>;

function read(key: string): string | null {
  try {
    return window.localStorage.getItem(key);
  } catch {
    return null; // storage blocked
  }
}

function write(key: string, value: string): void {
  try {
    window.localStorage.setItem(key, value);
  } catch {
    // storage blocked or full: the preference stays in memory for this tab
  }
}

function readHidden(): Hidden {
  try {
    const parsed: unknown = JSON.parse(read(HIDDEN_MONOS_KEY) ?? "{}");
    if (!parsed || typeof parsed !== "object" || Array.isArray(parsed)) return {};
    return Object.fromEntries(
      Object.entries(parsed).filter((entry): entry is [string, string[]] => Array.isArray(entry[1]) && entry[1].every((id) => typeof id === "string")),
    );
  } catch {
    return {};
  }
}

interface MonoPrefs {
  monosOnly: boolean;
  hidden: Hidden;
  setMonosOnly: (value: boolean) => void;
  setHidden: (desktopId: string, monoId: string, hidden: boolean) => void;
  unhideAll: (desktopId: string) => void;
}

export const useMonoPrefs = create<MonoPrefs>((set, get) => ({
  monosOnly: read(MONOS_ONLY_KEY) === "1",
  hidden: readHidden(),
  setMonosOnly: (value) => {
    write(MONOS_ONLY_KEY, value ? "1" : "0");
    set({ monosOnly: value });
  },
  setHidden: (desktopId, monoId, hide) => {
    const current = get().hidden[desktopId] ?? [];
    const next = hide ? [...new Set([...current, monoId])] : current.filter((id) => id !== monoId);
    const hidden = { ...get().hidden, [desktopId]: next };
    write(HIDDEN_MONOS_KEY, JSON.stringify(hidden));
    set({ hidden });
  },
  unhideAll: (desktopId) => {
    const hidden = { ...get().hidden, [desktopId]: [] };
    write(HIDDEN_MONOS_KEY, JSON.stringify(hidden));
    set({ hidden });
  },
}));

/** Re-reads the stored values (for tests, after seeding or clearing localStorage). */
export function reloadMonoPrefs(): void {
  useMonoPrefs.setState({ monosOnly: read(MONOS_ONLY_KEY) === "1", hidden: readHidden() });
}
