import { useEffect, useState } from "react";
import { liveRunRegistry } from "./workflowLiveRuns";

/**
 * Exactly one window drives workflows at a time. Every window runs the whole app, but a run
 * lives in the memory of the window that started it, so a second driver would sweep that run
 * to "interrupted", start a second run of the same workflow and be unable to stop the first.
 * The driver is whichever window holds a lease in localStorage, which same-origin windows
 * share. Other windows ask the driver through the workflow files.
 */
export const DRIVER_LEASE_KEY = "monocode.workflowDriver";
/** How often the holder renews the lease and the others look for it to be free. */
export const DRIVER_CHECK_MS = 10_000;
/** A lease not renewed for this long belongs to a window that is gone. */
export const DRIVER_STALE_MS = 30_000;

type Lease = { id: string; at: number };
type LeaseStorage = Pick<Storage, "getItem" | "setItem" | "removeItem">;

export type DriverLease = {
  /** Takes the lease if it is free, stale or already ours, or renews it. Whether we hold it. */
  claim(): boolean;
  /** Gives the lease up if we hold it, so another window can take over at once. */
  release(): void;
};

export function createDriverLease(options: {
  storage: LeaseStorage;
  now?: () => number;
  id?: string;
}): DriverLease {
  const { storage } = options;
  const now = options.now ?? Date.now;
  const id = options.id ?? crypto.randomUUID();
  const read = (): Lease | undefined => {
    try {
      const value = JSON.parse(storage.getItem(DRIVER_LEASE_KEY) ?? "null");
      return typeof value?.id === "string" && typeof value.at === "number"
        ? value
        : undefined;
    } catch {
      return undefined;
    }
  };
  return {
    claim() {
      try {
        const current = read();
        if (
          current &&
          current.id !== id &&
          now() - current.at <= DRIVER_STALE_MS
        )
          return false;
        storage.setItem(DRIVER_LEASE_KEY, JSON.stringify({ id, at: now() }));
        // Two windows can write at once; whoever wrote last holds it, and the other finds out here.
        return read()?.id === id;
      } catch {
        return false;
      }
    },
    release() {
      try {
        if (read()?.id === id) storage.removeItem(DRIVER_LEASE_KEY);
      } catch {
        // The lease goes stale on its own.
      }
    },
  };
}

/** Whether this window is the one that drives workflows, re-elected as windows come and go. */
export function useWorkflowDriver(): boolean {
  const [drives, setDrives] = useState(false);
  useEffect(() => {
    // No usable localStorage (missing, or access throws): this window cannot be elected.
    let lease: DriverLease;
    try {
      lease = createDriverLease({ storage: localStorage });
    } catch {
      return;
    }
    const check = () => setDrives(lease.claim());
    const release = () => lease.release();
    // A window that is going away takes its runs off the registry too: the runner's own
    // clean-up never gets to run, and the next driver should not have to wait out the 30 s.
    const leave = () => {
      release();
      liveRunRegistry()?.removeOwned();
    };
    check();
    const timer = window.setInterval(check, DRIVER_CHECK_MS);
    // A window that closes frees the lease, and the others hear of it at once.
    const onStorage = (event: StorageEvent) => {
      if (event.key === DRIVER_LEASE_KEY && event.newValue === null) check();
    };
    window.addEventListener("storage", onStorage);
    window.addEventListener("beforeunload", leave);
    window.addEventListener("pagehide", leave);
    return () => {
      window.clearInterval(timer);
      window.removeEventListener("storage", onStorage);
      window.removeEventListener("beforeunload", leave);
      window.removeEventListener("pagehide", leave);
      release();
    };
  }, []);
  return drives;
}
