import { telegramStatus, type TelegramStatus } from "./telegram";

const STATUS_TTL_MS = 60_000;

type Entry = { at: number; status: Promise<TelegramStatus | undefined> };

const entries = new Map<string, Entry>();

/**
 * Project status shared by the question hook, cached for a minute. A failed
 * call resolves to `undefined` and is never kept, so the next caller asks again.
 */
export function cachedTelegramStatus(
  project: string,
  at: number,
): Promise<TelegramStatus | undefined> {
  const cached = entries.get(project);
  if (cached && at - cached.at < STATUS_TTL_MS) return cached.status;
  const entry: Entry = {
    at,
    status: telegramStatus(project).then(
      (status) => status,
      () => {
        if (entries.get(project) === entry) entries.delete(project);
        return undefined;
      },
    ),
  };
  entries.set(project, entry);
  return entry.status;
}

/** Drops the cached status of one project, or of all projects. */
export function invalidateTelegramStatus(project?: string): void {
  if (project === undefined) entries.clear();
  else entries.delete(project);
}
