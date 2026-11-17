import type { UseQueryResult } from "@tanstack/react-query";
import type { ReactNode } from "react";
import { Button } from "@/ui/button";
import { DelayedSpinner } from "@/ui/DelayedSpinner";
import { Notice } from "@/ui/Notice";

/** Loading (spinner after 250 ms), error with Retry, or the content. */
export function QueryBoundary<T>({ query, label, children }: { query: UseQueryResult<T>; label: string; children: (data: T) => ReactNode }) {
  if (query.data !== undefined) return <>{children(query.data)}</>;
  if (query.isError) {
    return (
      <div className="flex flex-col items-start gap-3 py-2">
        <Notice variant="error" className="w-full">{`Could not load ${label}.`}</Notice>
        <Button type="button" variant="outline" size="sm" onClick={() => void query.refetch()}>
          Try again
        </Button>
      </div>
    );
  }
  return (
    <div className="flex min-h-13 items-center justify-center py-2">
      <DelayedSpinner label={`Loading ${label}`} />
    </div>
  );
}
