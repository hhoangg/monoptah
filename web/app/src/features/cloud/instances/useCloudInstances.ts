import type { ResultOf } from "@monoptah/protocol";
import { useLinkQuery } from "@/data";

export type CloudInstancesResult = ResultOf<"cloud.instances">;
export type CloudInstance = CloudInstancesResult["instances"][number];

/** Poll period while something is changing. The read never wakes an instance. */
export const INSTANCES_POLL_MS = 7_000;

/** True while an instance is starting or stopping, or a stop is being reported. */
export function isInstanceMoving(instance: Pick<CloudInstance, "state" | "stopReport">): boolean {
  return instance.state === "starting" || instance.state === "stopping" || instance.stopReport !== undefined;
}

/** Interval of the poll: modest while any instance is moving, off when idle. Never polls in a hidden tab. */
export function instancesRefetchInterval(data: CloudInstancesResult | undefined): number | false {
  return data?.instances.some(isInstanceMoving) ? INSTANCES_POLL_MS : false;
}

export function useCloudInstances() {
  return useLinkQuery("cloud.instances", {}, { refetchInterval: (query) => instancesRefetchInterval(query.state.data), refetchIntervalInBackground: false });
}
