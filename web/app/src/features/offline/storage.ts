// `navigator.storage.persist()` (architecture.md 3.4): asked once, after sign-in, so Safari's 7-day
// eviction of script-writable storage does not remove the device key and the cache of an installed PWA.
// The browser decides (it may refuse without a prompt); the answer is kept in memory only.

export type PersistentStorageState = "unknown" | "granted" | "denied" | "unsupported";

let state: PersistentStorageState = "unknown";
let pending: Promise<PersistentStorageState> | undefined;

/** Asks at most once per page load; later calls return the first answer. Never throws. */
export function requestPersistentStorage(): Promise<PersistentStorageState> {
  if (pending) return pending;
  const storage = typeof navigator === "undefined" ? undefined : navigator.storage;
  if (!storage || typeof storage.persist !== "function") {
    state = "unsupported";
    pending = Promise.resolve(state);
    return pending;
  }
  pending = storage
    .persist()
    .then((granted): PersistentStorageState => (granted ? "granted" : "denied"))
    .catch((): PersistentStorageState => "denied")
    .then((result) => {
      state = result;
      return result;
    });
  return pending;
}

/** What the browser answered, or "unknown" before it was asked. */
export function getPersistentStorageState(): PersistentStorageState {
  return state;
}

/** For tests. */
export function resetPersistentStorageState(): void {
  state = "unknown";
  pending = undefined;
}
