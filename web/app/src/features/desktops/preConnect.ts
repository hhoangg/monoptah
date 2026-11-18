// The pre-connect grant hook of the link (link/connection.ts `setPreConnectHook`). Owner: WS-M.
//
// Before the relay is joined the link asks this hook whether the device has a grant for the desktop. A device
// without a grant row is refused by the relay with close 4003, which used to be a race with `useLazyGrant`
// on the first open; now the grant is requested first, so the join sees it. The POST is idempotent. The first
// connect of a desktop in this page load always asks, even when the marker of storage.ts says this device asked
// before: the 7-day cleanup deletes a pending grant nobody confirmed, and without a new request the join would
// hit 4003 and end on "Access revoked". Later reconnects in the same load skip it when the marker is set. The
// request is shared with `useLazyGrant` while in flight (`requestOnce`).
//  - the server says the grant itself is revoked (409): "denied" (the link blocks). A 404 or 403 (a desktop this account
//    no longer owns, an unknown device) is "ok": the directory read ends it quietly or the relay refuses, as before;
//  - the request failed (network, server): "ok", the relay stays the authority and the link carries on as before.
import type { QueryClient } from "@tanstack/react-query";
import type { GrantCheck, PreConnectHook } from "@/link";
import { requestOnce } from "./useLazyGrant";
import { wasGrantRequested } from "./storage";

export function createGrantHook(queryClient: QueryClient): PreConnectHook {
  const askedThisLoad = new Set<string>();
  return async ({ desktopId, deviceId }): Promise<GrantCheck> => {
    const key = `${desktopId}:${deviceId}`;
    if (wasGrantRequested(desktopId, deviceId) && askedThisLoad.has(key)) return "ok";
    askedThisLoad.add(key);
    return (await requestOnce(queryClient, desktopId, deviceId)) === "revoked" ? "denied" : "ok";
  };
}
