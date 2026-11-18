// The send guard is not a hook, so it cannot read the conversation's live `cloud` field or the catalog card.
// The components of a mounted session (header chip, composer top) publish what they know here, keyed by
// desktop and session, and the guard reads every source (live field, card target state, card `cloudState`)
// and takes the most billable-cautious answer. Several components can publish for one session: each is its
// own writer, so the first one to unmount removes only its own entry.
import { useEffect, useId } from "react";
import type { CloudState } from "@monoptah/protocol";

/** What one writer knows: the kinds and states it saw, from the live field and from the catalog card. */
export interface LivePlacement {
  kinds: readonly string[];
  states: readonly CloudState[];
}

const writers = new Map<string, Map<string, LivePlacement>>();
const keyOf = (desktopId: string, sessionId: string) => `${desktopId}\u0000${sessionId}`;

export function publishPlacement(desktopId: string, sessionId: string, writerId: string, value: LivePlacement | undefined): void {
  const key = keyOf(desktopId, sessionId);
  const entries = writers.get(key) ?? new Map<string, LivePlacement>();
  if (value) entries.set(writerId, value);
  else entries.delete(writerId);
  if (entries.size > 0) writers.set(key, entries);
  else writers.delete(key);
}

/** Every kind and state published for the session by all mounted writers. */
export function getLivePlacement(desktopId: string, sessionId: string): { kinds: string[]; states: CloudState[] } {
  const kinds = new Set<string>();
  const states = new Set<CloudState>();
  for (const entry of writers.get(keyOf(desktopId, sessionId))?.values() ?? []) {
    for (const kind of entry.kinds) kinds.add(kind);
    for (const state of entry.states) states.add(state);
  }
  return { kinds: [...kinds], states: [...states] };
}

/** Publishes `value` for the session while the calling component is mounted. */
export function useLivePlacement(desktopId: string, sessionId: string, value: LivePlacement): void {
  const writerId = useId();
  const kinds = value.kinds.join(",");
  const states = value.states.join(",");
  useEffect(() => {
    publishPlacement(desktopId, sessionId, writerId, {
      kinds: kinds === "" ? [] : kinds.split(","),
      states: (states === "" ? [] : states.split(",")) as CloudState[],
    });
    return () => publishPlacement(desktopId, sessionId, writerId, undefined);
  }, [desktopId, sessionId, writerId, kinds, states]);
}

/** For tests. */
export function clearLivePlacements(): void {
  writers.clear();
}
