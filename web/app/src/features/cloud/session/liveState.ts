// The send guard is not a hook, so it cannot read the conversation's live `cloud` field. The components
// of a mounted session (header chip, composer top) publish the live state here, and the guard prefers it
// over the catalog card, which can be a poll behind a watch frame.
import { useEffect } from "react";
import type { CloudState } from "@monoptah/protocol";

const live = new Map<string, CloudState>();

export function getLiveCloudState(sessionId: string): CloudState | undefined {
  return live.get(sessionId);
}

export function setLiveCloudState(sessionId: string, state: CloudState | undefined): void {
  if (state) live.set(sessionId, state);
  else live.delete(sessionId);
}

/** Publishes `state` for `sessionId` while the calling component is mounted. */
export function useLiveCloudState(sessionId: string, state: CloudState | undefined): void {
  useEffect(() => {
    setLiveCloudState(sessionId, state);
  }, [sessionId, state]);
  useEffect(() => () => setLiveCloudState(sessionId, undefined), [sessionId]);
}

/** For tests. */
export function clearLiveCloudStates(): void {
  live.clear();
}
