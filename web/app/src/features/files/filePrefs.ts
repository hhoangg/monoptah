// Persisted preference of the file viewer: wrap long lines (`monoptah.files.wrap`, shared by the file viewer and the diff).
// Until the user chooses, wrap is off, except on a phone-width window (below 640px) where it is on.
import { create } from "zustand";

export const WRAP_KEY = "monoptah.files.wrap";

function narrowDefault(): boolean {
  return typeof window.matchMedia === "function" && window.matchMedia("(max-width: 639px)").matches;
}

function readWrap(): boolean {
  try {
    const stored = window.localStorage.getItem(WRAP_KEY);
    return stored === null ? narrowDefault() : stored === "1";
  } catch {
    return narrowDefault(); // storage blocked
  }
}

export const useFilePrefs = create<{ wrap: boolean; setWrap: (wrap: boolean) => void }>((set) => ({
  wrap: readWrap(),
  setWrap: (wrap) => {
    try {
      window.localStorage.setItem(WRAP_KEY, wrap ? "1" : "0");
    } catch {
      // storage blocked or full: the preference stays in memory for this tab
    }
    set({ wrap });
  },
}));

/** Re-reads the stored value (for tests, after seeding or clearing localStorage). */
export function reloadFilePrefs(): void {
  useFilePrefs.setState({ wrap: readWrap() });
}
