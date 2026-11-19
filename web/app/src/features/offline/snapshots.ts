// Persisted account snapshots (see accountCache.ts). Kept free of imports from features/accounts so
// that module can call `forgetAccountSnapshots` when a session ends without an import cycle.
import { getPersistAdapter } from "@/data";

export const ACCOUNT_ME_KEY = "account:me";
export const ACCOUNT_DESKTOPS_KEY = "account:desktops";

export async function saveSnapshot(key: string, value: unknown): Promise<void> {
  try {
    await getPersistAdapter().set(key, value);
  } catch {
    // A failed write is only a missed snapshot.
  }
}

export async function loadSnapshot<T>(key: string, parse: (value: unknown) => T): Promise<T | undefined> {
  try {
    const value = await getPersistAdapter().get(key);
    return value === undefined ? undefined : parse(value);
  } catch {
    return undefined;
  }
}

/** Removes both snapshots. Never throws. */
export async function forgetAccountSnapshots(): Promise<void> {
  try {
    await getPersistAdapter().set(ACCOUNT_ME_KEY, undefined);
    await getPersistAdapter().set(ACCOUNT_DESKTOPS_KEY, undefined);
  } catch {
    // Nothing more to do.
  }
}

