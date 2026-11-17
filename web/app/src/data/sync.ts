// Keeps the desktop data fresh and private (architecture.md 3.2, 3.3, 3.6):
// - the catalog (`workspace.list`) is refetched every 5 s while the document is visible and the link is
//   `ready` (mobile ConnectionProvider.tsx:157-160), when the link becomes `ready`, and on `visibilitychange`
//   to visible;
// - after a reconnect (`resynced`) the catalog is invalidated and each open conversation is read again with
//   `conversation.read` (the fallback of WorkspaceProvider.tsx:391-394); a mutation is never re-sent;
// - the catalog is written through the persist adapter, and a `revoked` link clears the persisted cache of that desktop only.
import { useQueryClient, type QueryClient } from "@tanstack/react-query";
import { useEffect } from "react";
import { useLinkConnection } from "@/link";
import type { LinkClient } from "./client.ts";
import { openConversationWatchers } from "./conversation.ts";
import { clearPersistedCache, loadPersistedCatalog, persistCatalog } from "./persist.ts";
import { catalogKey, desktopKey, linkQueryOptions } from "./queries.ts";

/** The catalog poll interval while visible and ready. */
export const CATALOG_POLL_MS = 5_000;

/** Methods whose cached results are the catalog (invalidated after a reconnect). */
const CATALOG_METHODS = new Set(["workspace.list", "sessions.list"]);

export interface LinkDataSyncOptions {
  pollMs?: number;
}

function isVisible(): boolean {
  return globalThis.document?.hidden !== true;
}

/** Starts the sync of one desktop; returns the function that stops it. Prefer `useLinkDataSync`. */
export function startLinkDataSync(client: LinkClient, queryClient: QueryClient, options: LinkDataSyncOptions = {}): () => void {
  const desktopId = client.desktopId;
  const isReady = () => client.store.getState().status === "ready";
  let stopped = false;

  const refreshCatalog = (): void => {
    if (stopped || !isReady()) return;
    queryClient.fetchQuery({ ...linkQueryOptions(desktopId, "workspace.list", {}, { connection: client }), staleTime: 0 }).catch(() => undefined);
  };

  // Show the persisted catalog while the first fetch is on its way. Never newer than a fetched one.
  void loadPersistedCatalog(desktopId).then((persisted) => {
    if (stopped || persisted === undefined) return;
    if (queryClient.getQueryData(catalogKey(desktopId)) === undefined) queryClient.setQueryData(catalogKey(desktopId), persisted, { updatedAt: 0 });
  });

  const clearForRevoked = (): void => {
    void clearPersistedCache(desktopId);
    queryClient.removeQueries({ queryKey: desktopKey(desktopId) });
  };
  if (client.store.getState().revoked) clearForRevoked();

  // Link queries that mounted before the link was `ready` failed with `not_ready` (or never ran): fetch
  // them now. Disabled queries (the conversation entries) are not "active" and are left alone; a fetch
  // already in flight is not cancelled.
  const refetchWaiting = (): void => {
    void queryClient.refetchQueries(
      {
        type: "active",
        predicate: (query) =>
          query.queryKey[0] === "link" &&
          query.queryKey[1] === desktopId &&
          query.queryKey[2] !== "conversation" &&
          (query.state.status === "error" || (query.state.status === "pending" && query.state.fetchStatus === "idle")),
      },
      { cancelRefetch: false, throwOnError: false },
    );
  };

  const unsubscribeStore = client.store.subscribe((state, previous) => {
    if (state.status === "ready" && previous.status !== "ready") {
      refreshCatalog();
      refetchWaiting();
    }
    if (state.revoked && !previous.revoked) clearForRevoked();
  });

  const timer = setInterval(() => {
    if (isVisible()) refreshCatalog();
  }, options.pollMs ?? CATALOG_POLL_MS);

  const onVisibility = (): void => {
    if (isVisible()) refreshCatalog();
  };
  globalThis.document?.addEventListener("visibilitychange", onVisibility);

  const unsubscribeResync = client.onResynced(() => {
    if (stopped) return;
    void queryClient.invalidateQueries({
      predicate: (query) => query.queryKey[0] === "link" && query.queryKey[1] === desktopId && CATALOG_METHODS.has(String(query.queryKey[2])),
      refetchType: "active",
    });
    refreshCatalog();
    refetchWaiting();
    for (const watcher of openConversationWatchers(queryClient, desktopId)) watcher.read().catch(() => undefined);
  });

  const unsubscribeCache = queryClient.getQueryCache().subscribe((event) => {
    if (event.type !== "updated" || event.action.type !== "success") return;
    const key = event.query.queryKey;
    if (key[0] === "link" && key[1] === desktopId && key[2] === "workspace.list" && !client.store.getState().revoked) {
      void persistCatalog(desktopId, event.query.state.data);
    }
  });

  if (isReady()) {
    refreshCatalog();
    refetchWaiting();
  }

  return () => {
    stopped = true;
    clearInterval(timer);
    globalThis.document?.removeEventListener("visibilitychange", onVisibility);
    unsubscribeStore();
    unsubscribeResync();
    unsubscribeCache();
  };
}

const syncs = new WeakMap<QueryClient, Map<string, { refs: number; stop: () => void }>>();

/** Ref counted `startLinkDataSync`: one sync per query client and desktop however many screens ask for it. */
export function acquireLinkDataSync(client: LinkClient, queryClient: QueryClient, options?: LinkDataSyncOptions): () => void {
  let map = syncs.get(queryClient);
  if (!map) {
    map = new Map();
    syncs.set(queryClient, map);
  }
  let entry = map.get(client.desktopId);
  if (!entry) {
    entry = { refs: 0, stop: startLinkDataSync(client, queryClient, options) };
    map.set(client.desktopId, entry);
  }
  entry.refs += 1;
  const owned = entry;
  const registry = map;
  let released = false;
  return () => {
    if (released) return;
    released = true;
    owned.refs -= 1;
    if (owned.refs === 0) {
      owned.stop();
      registry.delete(client.desktopId);
    }
  };
}

/**
 * Mounts the sync for the desktop of the nearest `LinkProvider`. `useCatalog` and `useConversation`
 * call it; the `/d/$desktopId` layout route should also call it once so the revoked clean-up runs
 * even when no catalog screen is mounted.
 */
export function useLinkDataSync(): void {
  const client = useLinkConnection();
  const queryClient = useQueryClient();
  useEffect(() => acquireLinkDataSync(client, queryClient), [client, queryClient]);
}
