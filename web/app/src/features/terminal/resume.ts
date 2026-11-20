// What a reload within the grace window needs to find the same shell again: the terminal id and the offset of the next
// output byte. Never the output (security.md "Terminal"): sessionStorage holds ids and counters only, and it dies with the tab.

export interface ResumeEntry {
  terminalId: string;
  /** The first output byte not yet shown. */
  next: number;
  cwd: string;
  idleTimeoutMs: number;
  maxDurationMs: number;
  /** Epoch ms the shell was opened, for the remaining time hints. */
  openedAt: number;
}

export interface ResumeStore {
  load(): ResumeEntry | null;
  save(entry: ResumeEntry): void;
  clear(): void;
}

export const RESUME_KEY_PREFIX = "monoptah.terminal.";

function isEntry(value: unknown): value is ResumeEntry {
  if (typeof value !== "object" || value === null) return false;
  const v = value as Record<string, unknown>;
  const count = (x: unknown): boolean => typeof x === "number" && Number.isFinite(x) && x >= 0;
  return typeof v["terminalId"] === "string" && v["terminalId"].length > 0 && count(v["next"]) && typeof v["cwd"] === "string" && count(v["idleTimeoutMs"]) && count(v["maxDurationMs"]) && count(v["openedAt"]);
}

/** One entry per `scope` (a desktop and a terminal tab, `<desktopId>.<terminalId>`) in `sessionStorage`; every failure (storage off, private mode, bad JSON) reads as "nothing saved". */
export function sessionResumeStore(scope: string, storage: Pick<Storage, "getItem" | "setItem" | "removeItem"> | null = safeSession()): ResumeStore {
  const key = `${RESUME_KEY_PREFIX}${scope}`;
  return {
    load() {
      try {
        const raw = storage?.getItem(key);
        if (!raw) return null;
        const parsed: unknown = JSON.parse(raw);
        return isEntry(parsed) ? parsed : null;
      } catch {
        return null;
      }
    },
    save(entry) {
      try {
        storage?.setItem(key, JSON.stringify({ terminalId: entry.terminalId, next: entry.next, cwd: entry.cwd, idleTimeoutMs: entry.idleTimeoutMs, maxDurationMs: entry.maxDurationMs, openedAt: entry.openedAt }));
      } catch {
        // A full or disabled storage only loses the reattach after a reload.
      }
    },
    clear() {
      try {
        storage?.removeItem(key);
      } catch {
        // Nothing to clear.
      }
    },
  };
}

function safeSession(): Storage | null {
  try {
    return typeof sessionStorage === "undefined" ? null : sessionStorage;
  } catch {
    return null;
  }
}
