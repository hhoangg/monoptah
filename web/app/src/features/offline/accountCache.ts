// Account snapshots for an offline start (F-05, architecture.md 3.6): the last `GET /api/me` and
// `GET /api/desktops` answers go through the persist adapter (encrypted at rest), so a cold start
// without a network can pass the root auth guard and show the desktops list.
//
// Only a failed network (ApiError status 0, code `network`) uses a snapshot. A 401 never does, and the
// snapshots are wiped when the session ends (`forgetAccountSnapshots`), so signing out leaves nothing
// that could pass for a session.
import type { QueryClient } from "@tanstack/react-query";
import { DesktopsResponse, MeResponse } from "@monoptah/protocol";
import { desktopsQuery, meQuery } from "@/features/accounts/queries";
import { ACCOUNT_DESKTOPS_KEY, ACCOUNT_ME_KEY, loadSnapshot, saveSnapshot } from "./snapshots";
import { requestPersistentStorage } from "./storage";

type Me = ReturnType<typeof MeResponse.parse>;

/**
 * Puts the snapshots into the query cache and returns the signed-in account, or undefined when there
 * is none (never signed in, a recovery-only session, or an unreadable snapshot).
 */
export async function restoreAccountSnapshots(queryClient: QueryClient): Promise<Me | undefined> {
  const me = await loadSnapshot(ACCOUNT_ME_KEY, (value) => MeResponse.parse(value));
  if (!me || me.recoveryOnly) return undefined;
  queryClient.setQueryData(meQuery.queryKey, me);
  const desktops = await loadSnapshot(ACCOUNT_DESKTOPS_KEY, (value) => DesktopsResponse.parse(value));
  if (desktops && queryClient.getQueryData(desktopsQuery.queryKey) === undefined) queryClient.setQueryData(desktopsQuery.queryKey, desktops);
  return me;
}

/**
 * Writes a snapshot whenever `me` or `desktops` load successfully, and asks for persistent storage once
 * the first `me` has loaded (the user is signed in). Returns the unsubscribe function.
 */
export function startAccountPersistence(queryClient: QueryClient): () => void {
  return queryClient.getQueryCache().subscribe((event) => {
    if (event.type !== "updated" || event.action.type !== "success") return;
    const [head] = event.query.queryKey;
    const data = event.query.state.data;
    if (head === meQuery.queryKey[0] && event.query.queryKey.length === 1) {
      if (!MeResponse.safeParse(data).success || (data as Me).recoveryOnly) return;
      void saveSnapshot(ACCOUNT_ME_KEY, data);
      void requestPersistentStorage();
    } else if (head === desktopsQuery.queryKey[0] && event.query.queryKey.length === 1) {
      if (DesktopsResponse.safeParse(data).success) void saveSnapshot(ACCOUNT_DESKTOPS_KEY, data);
    }
  });
}
