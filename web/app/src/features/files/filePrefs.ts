// Persisted preference of the file viewer: wrap long lines (`monoptah.files.wrap`, default off).
import { create } from "zustand";

export const WRAP_KEY = "monoptah.files.wrap";

function readWrap(): boolean {
  try {
    return window.localStorage.getItem(WRAP_KEY) === "1";
  } catch {
    return false; // storage blocked
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
