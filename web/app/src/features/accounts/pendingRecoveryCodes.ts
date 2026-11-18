// State shared by the screens that show freshly issued recovery codes. In memory only: no code is kept here.
//  - `batch`: the batch number the Worker returned with the codes on screen. "I saved these" sends it, and the
//    Worker acknowledges only that batch (a newer regenerate in another tab is not acknowledged by mistake).
//  - `shown`: true while codes are on screen waiting for the user. The Worker clears `recoveryAckAt` when it issues
//    codes, so any refetch of `me` would send /settings to the forced regenerate screen and unmount the codes
//    before they are saved; the route keeps the page while this is true.
import { useSyncExternalStore } from "react";

let batch: number | null = null;
let shown = false;
const listeners = new Set<() => void>();

function emit(): void {
  for (const listener of listeners) listener();
}

export function setRecoveryBatch(value: number | null): void {
  batch = value;
}

export function getRecoveryBatch(): number | null {
  return batch;
}

export function setRecoveryCodesShown(value: boolean): void {
  if (shown === value) return;
  shown = value;
  emit();
}

function subscribe(listener: () => void): () => void {
  listeners.add(listener);
  return () => listeners.delete(listener);
}

/** True while freshly issued recovery codes are on screen. */
export function useRecoveryCodesShown(): boolean {
  return useSyncExternalStore(subscribe, () => shown, () => false);
}
