// "This item is not in the catalog" is only worth telling the user after the catalog was fetched again once this screen
// was open: a push can arrive for a session or Mono the cached (or polled) catalog does not list yet. When the item is
// missing on mount the catalog is refetched once; the answer counts only if that fetch finished after the mount.
import { useEffect, useRef, useState } from "react";

interface CatalogQueryLike {
  isFetching: boolean;
  dataUpdatedAt: number;
  refetch(): Promise<unknown>;
}

/** True once the item is still missing after a catalog fetch that completed after this screen mounted. */
export function useConfirmedMissing(query: CatalogQueryLike, loaded: boolean, missing: boolean, now: () => number = Date.now): boolean {
  const [mountedAt] = useState(now);
  const asked = useRef(false);
  const { refetch } = query;
  useEffect(() => {
    if (!loaded || !missing || asked.current) return;
    asked.current = true;
    void refetch().catch(() => undefined);
  }, [loaded, missing, refetch]);
  return loaded && missing && !query.isFetching && query.dataUpdatedAt >= mountedAt;
}
