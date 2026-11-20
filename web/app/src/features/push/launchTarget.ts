// The page side of the pending notification target (sw/pendingTarget.ts): a notification tap that could not be
// delivered as a message (cold start, iOS standalone, a page still booting) is taken once here and routed.
import { takePendingTap, type PendingStore } from "@/sw/pendingTarget";

export interface LaunchDeps {
  store: PendingStore;
  /** Waits until the first route is resolved, so a `/` redirect to the last place cannot override the target. */
  ready(): Promise<unknown>;
  /** The current path, search and hash. */
  currentHref(): string;
  /** `at` is the tap id: the same tap reaching the page twice (message and saved target) must route once. */
  navigate(url: string, at: number): Promise<unknown> | void;
  now(): number;
}

/** Routes the pending target if there is a fresh one. Returns the URL it went to, or null. Safe to call often. */
export async function consumePendingTarget(deps: LaunchDeps): Promise<string | null> {
  const tap = await takePendingTap(deps.store, deps.now());
  if (tap === null) return null;
  await deps.ready();
  if (deps.currentHref() !== tap.url) await deps.navigate(tap.url, tap.at);
  return tap.url;

}

/** Remembers which taps were routed, so one tap that arrives as a message and as a saved target goes once. */
export function createTapGate(limit = 16) {
  const seen: number[] = [];
  return {
    /** True the first time `at` is offered (or always, when there is no id); false for a tap already routed. */
    first(at: number | undefined): boolean {
      if (at === undefined) return true;
      if (seen.includes(at)) return false;
      seen.push(at);
      if (seen.length > limit) seen.shift();
      return true;
    },
  };
}
