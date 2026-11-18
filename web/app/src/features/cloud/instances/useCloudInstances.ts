import { useRef } from "react";
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
export function instancesRefetchInterval(data: CloudInstancesResult | undefined, options: { watching?: boolean; elapsedMs?: number } = {}): number | false {
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

export function useCloudInstances({ watching, epoch = 0 }: UseCloudInstancesOptions = {}) {
  const startedAt = useRef<number | null>(null);
  const lastEpoch = useRef(epoch);
  return useLinkQuery(
    "cloud.instances",
    {},
    {
      refetchInterval: (query) => {
        // A new own action restarts the polling clock.
        if (lastEpoch.current !== epoch) {
          lastEpoch.current = epoch;
          startedAt.current = null;
        }
        if (query.state.status === "error" && query.state.errorUpdateCount >= INSTANCES_MAX_ERRORS) return false;
        const own = watching?.(query.state.data) ?? false;
        const idle = instancesRefetchInterval(query.state.data, { watching: own });
        if (idle === false) {
          startedAt.current = null;
          return false;
        }
        startedAt.current ??= Date.now();
        return instancesRefetchInterval(query.state.data, { watching: own, elapsedMs: Date.now() - startedAt.current });
      },
      refetchIntervalInBackground: false,
    },
  );
}
