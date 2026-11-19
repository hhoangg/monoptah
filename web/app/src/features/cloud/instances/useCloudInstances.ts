import { useRef, useState } from "react";
import type { ResultOf } from "@monoptah/protocol";
import { useLinkQuery } from "@/data";

export type CloudInstancesResult = ResultOf<"cloud.instances">;
export type CloudInstance = CloudInstancesResult["instances"][number];

/** Poll period while something is changing. The read never wakes an instance. */
export const INSTANCES_POLL_MS = 7_000;
/** After this long of polling without a pause the period grows to `INSTANCES_SLOW_POLL_MS`. */
export const INSTANCES_FAST_WINDOW_MS = 120_000;
export const INSTANCES_SLOW_POLL_MS = 30_000;
/** Polling stops for good after this long (an instance stuck in `starting` or `stillRunning` must not poll forever). */
export const INSTANCES_POLL_CAP_MS = 600_000;
/** While the desktop answers `loading: true` the list is partial: first re-poll after this, doubling up to the max. */
export const LOADING_POLL_MIN_MS = 1_000;
export const LOADING_POLL_MAX_MS = 8_000;
/** Re-polling a `loading: true` answer stops after this long. */
export const LOADING_POLL_CAP_MS = 60_000;

/** The wait before the next re-poll of a `loading` answer; `attempt` counts the re-polls so far (0 first). */
export function loadingRefetchInterval(attempt: number, elapsedMs: number): number | false {
  if (elapsedMs >= LOADING_POLL_CAP_MS) return false;
  return Math.min(LOADING_POLL_MAX_MS, LOADING_POLL_MIN_MS * 2 ** Math.min(attempt, 10));
}

/** After this device's own Stop or Resume the list keeps polling this long even if the cached overview has not changed yet. */
export const OWN_ACTION_WATCH_MS = 150_000;
/** Polling stops after this many failed refreshes in a row. */
export const INSTANCES_MAX_ERRORS = 3;

/** True while an instance is starting or stopping, or a stop is being reported. */
export function isInstanceMoving(instance: Pick<CloudInstance, "state" | "stopReport">): boolean {
  return instance.state === "starting" || instance.state === "stopping" || instance.stopReport !== undefined;
}

/**
 * Interval of the poll. `elapsedMs` is how long polling has run without a pause: modest at first, slower
 * later, none after the cap. Off when nothing moves and no own action is being watched.
 */
export function instancesRefetchInterval(
  data: CloudInstancesResult | undefined,
  options: { watching?: boolean; elapsedMs?: number; loadingAttempt?: number; loadingElapsedMs?: number } = {},
): number | false {
  if (data?.loading === true) return loadingRefetchInterval(options.loadingAttempt ?? 0, options.loadingElapsedMs ?? 0);
  const moving = options.watching === true || (data?.instances.some(isInstanceMoving) ?? false);
  if (!moving) return false;
  const elapsed = options.elapsedMs ?? 0;
  if (elapsed >= INSTANCES_POLL_CAP_MS) return false;
  return elapsed < INSTANCES_FAST_WINDOW_MS ? INSTANCES_POLL_MS : INSTANCES_SLOW_POLL_MS;
}

export interface UseCloudInstancesOptions {
  /** True while this device's own Stop or Resume has not shown its effect in `data` yet (see `OWN_ACTION_WATCH_MS`). */
  watching?: (data: CloudInstancesResult | undefined) => boolean;
  /** Change it to restart the polling clock (a new own action after the poll gave up). */
  epoch?: number;
}

/**
 * The instance list with its poll. `paused` is true when polling stopped although the list still looks like it
 * is changing (the cap, or `INSTANCES_MAX_ERRORS` failed refreshes in a row): the screen offers a refresh.
 */
export function useCloudInstancesPolled({ watching, epoch = 0 }: UseCloudInstancesOptions = {}) {
  const startedAt = useRef<number | null>(null);
  // A `loading: true` answer is re-polled with its own backoff, from the first such answer to the first complete one.
  const loadingFrom = useRef<{ at: number; updates: number } | null>(null);
  const lastEpoch = useRef(epoch);
  // `errorUpdateCount` never resets, so the streak is counted from its value at the last good refresh.
  const errorBase = useRef(0);
  const [paused, setPaused] = useState(false);
  const query = useLinkQuery(
    "cloud.instances",
    {},
    {
      refetchInterval: (query) => {
        // A new own action restarts the polling clock and the error streak.
        if (lastEpoch.current !== epoch) {
          lastEpoch.current = epoch;
          startedAt.current = null;
          errorBase.current = query.state.errorUpdateCount;
        }
        if (query.state.status === "success") errorBase.current = query.state.errorUpdateCount;
        if (query.state.data?.loading === true) {
          loadingFrom.current ??= { at: Date.now(), updates: query.state.dataUpdateCount };
          const next = instancesRefetchInterval(query.state.data, {
            loadingAttempt: query.state.dataUpdateCount - loadingFrom.current.updates,
            loadingElapsedMs: Date.now() - loadingFrom.current.at,
          });
          queueMicrotask(() => setPaused(next === false));
          return next;
        }
        loadingFrom.current = null;
        const own = watching?.(query.state.data) ?? false;
        const wants = instancesRefetchInterval(query.state.data, { watching: own });
        let next: number | false = false;
        if (wants === false) {
          startedAt.current = null;
        } else if (query.state.errorUpdateCount - errorBase.current >= INSTANCES_MAX_ERRORS) {
          next = false;
        } else {
          startedAt.current ??= Date.now();
          next = instancesRefetchInterval(query.state.data, { watching: own, elapsedMs: Date.now() - startedAt.current });
        }
        const stopped = wants !== false && next === false;
        queueMicrotask(() => setPaused(stopped));
        return next;
      },
      refetchIntervalInBackground: false,
    },
  );
  return { query, paused };
}

export function useCloudInstances(options: UseCloudInstancesOptions = {}) {
  return useCloudInstancesPolled(options).query;
}
