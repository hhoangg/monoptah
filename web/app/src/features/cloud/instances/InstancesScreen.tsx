// U-17: the cloud instance list of one desktop. Read-only data (`cloud.instances` never wakes an
// instance); Stop and Resume are the only actions, both control and both confirmed.
//
// After this device's own Stop or Resume the list keeps polling for a bounded window (OWN_ACTION_WATCH_MS) and
// that row's actions wait, because the desktop's overview is cached and can still show the old state for a while.
// A second tap would otherwise send a second billable request.
import { useEffect, useState } from "react";
import type { CloudState } from "@monoptah/protocol";
import { useLinkState } from "@/link/LinkProvider";
import { Button } from "@/ui/button";
import { DelayedSpinner } from "@/ui/DelayedSpinner";
import { Notice } from "@/ui/Notice";
import { otherLabel } from "./copy";
import { InstanceRow } from "./InstanceRow";
import { OWN_ACTION_WATCH_MS, useCloudInstancesPolled, type CloudInstance } from "./useCloudInstances";

const keyOf = (instance: Pick<CloudInstance, "kind" | "instance">) => `${instance.kind}:${instance.instance}`;

/** What the row looked like when this device acted: the action is settled once the list shows something else. */
interface OwnAction {
  state: CloudState;
  stopReport: CloudInstance["stopReport"];
  until: number;
}

export function InstancesScreen({ desktopId }: { desktopId: string }) {
  const readOnly = useLinkState((state) => state.permission !== "control");
  const [own, setOwn] = useState<Record<string, OwnAction>>({});
  const [epoch, setEpoch] = useState(0);
  // The own action of a row is pending while the list still shows the row as it was and the window has not ended.
  const isPending = (instance: CloudInstance) => {
    const action = own[keyOf(instance)];
    return !!action && action.state === instance.state && action.stopReport === instance.stopReport;
  };
  const { query, paused } = useCloudInstancesPolled({ watching: (data) => data?.instances.some(isPending) ?? false, epoch });
  // The `warning` of the last stop per instance, shown on its row until the user leaves the screen.
  const [warnings, setWarnings] = useState<Record<string, string>>({});

  // Forget an own action when its window ends.
  useEffect(() => {
    const untils = Object.values(own).map((action) => action.until);
    if (untils.length === 0) return;
    const timer = setTimeout(() => setOwn((current) => Object.fromEntries(Object.entries(current).filter(([, action]) => action.until > Date.now()))), Math.max(0, Math.min(...untils) - Date.now()) + 5);
    return () => clearTimeout(timer);
  }, [own]);

  if (query.data === undefined) {
    if (query.isError) {
      return (
        <div className="flex flex-col items-start gap-3 py-2">
          <Notice variant="error" className="w-full">
            Could not load the cloud instances.
          </Notice>
          <Button type="button" variant="outline" size="sm" onClick={() => void query.refetch()}>
            Try again
          </Button>
        </div>
      );
    }
    return (
      <div className="flex min-h-13 items-center justify-center py-2">
        <DelayedSpinner label="Loading cloud instances" />
      </div>
    );
  }

  const { instances, otherCount } = query.data;
  const watch = (instance: CloudInstance) => {
    setOwn((current) => ({ ...current, [keyOf(instance)]: { state: instance.state, stopReport: instance.stopReport, until: Date.now() + OWN_ACTION_WATCH_MS } }));
    setEpoch((current) => current + 1);
    void query.refetch();
  };
  const onStopped = (instance: CloudInstance, warning: string | undefined) => {
    setWarnings((current) => {
      const next = { ...current };
      if (warning) next[keyOf(instance)] = warning;
      else delete next[keyOf(instance)];
      return next;
    });
    watch(instance);
  };
  return (
    <div className="flex flex-col gap-4">
      {query.isError ? <Notice variant="error">Could not refresh the list. Showing the last result.</Notice> : null}
      {paused ? (
        <Notice>
          Updates paused. The list no longer refreshes by itself.{" "}
          <Button
            type="button"
            variant="outline"
            size="sm"
            onClick={() => {
              setEpoch((current) => current + 1);
              void query.refetch();
            }}
          >
            Refresh
          </Button>
        </Notice>
      ) : null}
      {instances.length === 0 && query.data.loading === true && !paused ? (
        <div className="flex min-h-13 items-center justify-center py-2">
          <DelayedSpinner label="Loading cloud instances" />
        </div>
      ) : instances.length === 0 ? (
        <p data-testid="cloud-empty" className="py-6 text-center text-body text-muted-text">
          No cloud instances yet. Instances appear here when a session runs in the cloud.
        </p>
      ) : (
        <ul aria-label="Cloud instances" className="flex flex-col gap-3">
          {instances.map((instance) => (
            <InstanceRow
              key={keyOf(instance)}
              desktopId={desktopId}
              instance={instance}
              readOnly={readOnly}
              pending={isPending(instance)}
              warning={warnings[keyOf(instance)]}
              onStopped={onStopped}
              onResumed={watch}
              nowMs={query.dataUpdatedAt}
            />
          ))}
        </ul>
      )}
      {otherCount ? (
        <p data-testid="cloud-other" className="px-1 text-small text-muted-text">
          {otherLabel(otherCount)}
        </p>
      ) : null}
    </div>
  );
}
