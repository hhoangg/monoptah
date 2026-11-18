// The pre-connect grant hook of the link (link/connection.ts `setPreConnectHook`). Owner: WS-M.
//
// Before the relay is joined the link asks this hook whether the device has a grant for the desktop. A device
// without a grant row is refused by the relay with close 4003, which used to be a race with `useLazyGrant`
// on the first open; now the grant is requested first, so the join sees it. The POST is skipped when this device
// already asked (the marker of storage.ts) and shared with `useLazyGrant` while in flight (`requestOnce`).
//  - the server says the grant itself is revoked (409): "denied" (the link blocks). A 404 or 403 (a desktop this account
//    no longer owns, an unknown device) is "ok": the directory read ends it quietly or the relay refuses, as before;
//  - the request failed (network, server): "ok", the relay stays the authority and the link carries on as before.
import type { QueryClient } from "@tanstack/react-query";
import type { GrantCheck, PreConnectHook } from "@/link";
import { requestOnce } from "./useLazyGrant";
import { wasGrantRequested } from "./storage";

export function createGrantHook(queryClient: QueryClient): PreConnectHook {
  return async ({ desktopId, deviceId }): Promise<GrantCheck> => {
    if (wasGrantRequested(desktopId, deviceId)) return "ok";
    return (await requestOnce(queryClient, desktopId, deviceId)) === "revoked" ? "denied" : "ok";
  };
}
