// U-17: the cloud instance list of one desktop. Read-only data (`cloud.instances` never wakes an
// instance); Stop and Resume are the only actions, both control and both confirmed.
import { useState } from "react";
import { useLinkState } from "@/link/LinkProvider";
import { Button } from "@/ui/button";
import { DelayedSpinner } from "@/ui/DelayedSpinner";
import { Notice } from "@/ui/Notice";
import { otherLabel } from "./copy";
import { InstanceRow } from "./InstanceRow";
import { useCloudInstances, type CloudInstance } from "./useCloudInstances";

export function InstancesScreen({ desktopId }: { desktopId: string }) {
  const query = useCloudInstances();
  const readOnly = useLinkState((state) => state.permission !== "control");
  // The `warning` of the last stop per instance, shown on its row until the user leaves the screen.
  const [warnings, setWarnings] = useState<Record<string, string>>({});

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
  const onStopped = (instance: CloudInstance, warning: string | undefined) =>
    setWarnings((current) => {
      const next = { ...current };
      if (warning) next[instance.instance] = warning;
      else delete next[instance.instance];
      return next;
    });

  return (
    <div className="flex flex-col gap-4">
      {query.isError ? <Notice variant="error">Could not refresh the list. Showing the last result.</Notice> : null}
      {instances.length === 0 ? (
        <p data-testid="cloud-empty" className="py-6 text-center text-body text-muted-text">
          No cloud instances yet. Instances appear here when a session runs in the cloud.
        </p>
      ) : (
        <ul aria-label="Cloud instances" className="flex flex-col gap-3">
          {instances.map((instance) => (
            <InstanceRow
              key={`${instance.kind}:${instance.instance}`}
              desktopId={desktopId}
              instance={instance}
              readOnly={readOnly}
              warning={warnings[instance.instance]}
              onStopped={onStopped}
              onChanged={() => void query.refetch()}
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
