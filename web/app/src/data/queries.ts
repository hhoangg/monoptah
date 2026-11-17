// Query keys and option factories for desktop data (architecture.md 3.2). The link keys get
// `staleTime: Infinity`, `networkMode: 'always'` and `gcTime` 10 min per query here, not in the shared
// query client (owned by the Worker API data). Freshness comes from the link: watches, the catalog sync
// and the resync after a reconnect (sync.ts).
import { queryOptions, useQuery, type QueryKey, type UseQueryOptions } from "@tanstack/react-query";
import type { MethodName, ParamsOf, ResultOf } from "@monoptah/protocol";
import { getLinkConnection, useLinkDesktopId, type LinkRequestError } from "@/link";
import type { LinkClient } from "./client.ts";
import { useLinkDataSync } from "./sync.ts";

export const LINK_GC_TIME_MS = 10 * 60_000;

/** `['link', desktopId, method, params]`. */
export function linkKey<M extends MethodName>(desktopId: string, method: M, params: ParamsOf<M>) {
  return ["link", desktopId, method, params] as const;
}

/** `['link', desktopId, 'conversation', conversationId]`: a `ConversationState` (conversation.ts). */
export function conversationKey(desktopId: string, conversationId: string) {
  return ["link", desktopId, "conversation", conversationId] as const;
}

/** Every query of one desktop (used to drop the cache on revoke). */
export function desktopKey(desktopId: string) {
  return ["link", desktopId] as const;
}

/** The catalog query: `workspace.list`. */
export function catalogKey(desktopId: string) {
  return linkKey(desktopId, "workspace.list", {});
}

/** Defaults of every link query. */
export const LINK_QUERY_DEFAULTS = {
  staleTime: Infinity,
  networkMode: "always",
  gcTime: LINK_GC_TIME_MS,
  // A request is never queued or re-sent by the link; refreshing is the job of the sync, not of retries.
  retry: false,
  refetchOnWindowFocus: false,
  refetchOnReconnect: false,
} as const;

export interface LinkQueryExtras {
  /** The connection to use (default: the singleton of `desktopId`). */
  connection?: LinkClient;
}

/**
 * Request adapter: `queryFn` calls `connection.request(method, params)` and returns the result, typed with
 * `ParamsOf` and `ResultOf` of the protocol.
 */
export function linkQueryOptions<M extends MethodName>(desktopId: string, method: M, params: ParamsOf<M>, extras: LinkQueryExtras = {}) {
  return queryOptions<ResultOf<M>, LinkRequestError, ResultOf<M>, QueryKey>({
    ...LINK_QUERY_DEFAULTS,
    queryKey: linkKey(desktopId, method, params),
    queryFn: ({ signal }) => (extras.connection ?? getLinkConnection(desktopId)).request(method, params, { signal }),
  });
}

/** `useQuery` over `linkQueryOptions` for the desktop of the nearest `LinkProvider`. */
export function useLinkQuery<M extends MethodName>(
  method: M,
  params: ParamsOf<M>,
  options: Omit<UseQueryOptions<ResultOf<M>, LinkRequestError, ResultOf<M>, QueryKey>, "queryKey" | "queryFn"> = {},
) {
  const desktopId = useLinkDesktopId();
  // The sync refetches this query when the link becomes `ready` after it failed with `not_ready`.
  useLinkDataSync();
  return useQuery({ ...linkQueryOptions(desktopId, method, params), ...options });
}

/** The catalog (`workspace.list`) of the nearest desktop. `useLinkDataSync()` keeps it fresh. */
export function useCatalog() {
  useLinkDataSync();
  return useLinkQuery("workspace.list", {});
}
