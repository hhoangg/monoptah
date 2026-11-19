// Ties what this browser keeps on disk to the account that created it (security review 2, M1 and M2).
// The encrypted content cache (catalog and turns), the account snapshots and the push subscription are
// per browser, not per account. Without a binding, a session that ends without "Sign out" (idle or absolute
// expiry, revoked elsewhere) or another account signing in on the same profile would inherit them.
//
//  - `bindDeviceToUser(userId)` runs when a `me` answer arrives. A different (or unknown) owner wipes the
//    cache and rotates its key, removes the browser's push subscription and drops the Pause all record.
//  - `releaseDevice()` runs when a session ends (Sign out, a 401 anywhere) and does the same wipe.
//
// The owner marker is only the opaque user id in localStorage (not a secret); a cleared marker means
// "unknown owner", which wipes on the next sign-in. Never throws.
import { clearPersistedCache } from "@/data";
import { forgetPushLocally, resubscribeIfVapidKeyChanged } from "@/features/push/subscribe";
import { clearPauseRecord } from "@/features/push/pauseRecord";
import { loadPushState } from "@/sw/pushState";

export const OWNER_KEY = "monoptah.owner";

function readOwner(): string | null {
  try {
    return localStorage.getItem(OWNER_KEY);
  } catch {
    return null;
  }
}

function writeOwner(userId: string | null): void {
  try {
    if (userId === null) localStorage.removeItem(OWNER_KEY);
    else localStorage.setItem(OWNER_KEY, userId);
  } catch {
    // Blocked storage: the owner stays unknown and the next sign-in wipes again, which is the safe side.
  }
}

/** Wipes the encrypted cache (and rotates its key), the push subscription and the pause record. */
async function wipe(): Promise<void> {
  // The cache wipe is queued first so no later read can see the old records.
  const cache = clearPersistedCache();
  clearPauseRecord();
  await Promise.all([cache, forgetPushLocally()]);
}

let boundTo: string | null = null;

/** Call with `user.id` of every successful `me`. A repeat for the same user in one page load is a no-op. */
export async function bindDeviceToUser(userId: string): Promise<void> {
  if (boundTo === userId) return;
  boundTo = userId;
  try {
    const owner = readOwner();
    if (owner !== userId) {
      writeOwner(userId);
      await wipe();
      return;
    }
    // Same owner: a push state of another account (or of an old build) still has to go.
    const push = await loadPushState().catch(() => null);
    if (push && push.userId !== userId) await forgetPushLocally();
    else if (push) void resubscribeIfVapidKeyChanged(userId);
  } catch {
    // Nothing more to do.
  }
}

/** Call when a session ends. The next `me` binds the device again. */
export async function releaseDevice(): Promise<void> {
  boundTo = null;
  writeOwner(null);
  try {
    await wipe();
  } catch {
    // Nothing more to do.
  }
}

/** Test helper: forget the in-memory binding. */
export function resetDeviceBindingForTests(): void {
  boundTo = null;
}
