// Query options and call helpers for the account API (architecture.md 3.2). Screens use the
// query options with `useQuery` and the functions in mutations; every ceremony function returns a
// `cancelled` result when the user dismisses the browser prompt instead of throwing.
import { queryOptions, type QueryClient } from "@tanstack/react-query";
import type { MeResponse, PushPrefsResponse, User } from "@monoptah/protocol";
import { forgetAccountSnapshots } from "@/features/offline/snapshots";
import { ApiError, apiFetch } from "./api";
import { endpoints } from "./endpoints";
import { releaseDevice } from "./deviceBinding";
import { clearLowCodes } from "./lowCodes";
import { resetChatDrafts } from "@/features/composer/chatDraft";
import { resetNewSessionDrafts } from "@/features/sessions/newSessionDraft";
import { getRecoveryBatch, setRecoveryBatch } from "./pendingRecoveryCodes";
import { unsubscribeFromPush } from "@/features/push/subscribe";
import { getDeviceKeyInfo } from "@/link/keys";
import { createPasskey, getPasskey } from "./webauthn";

type Me = ReturnType<typeof MeResponse.parse>;

// --- Queries ---

export const ME_KEY = ["me"] as const;

/** 401 means "signed out": never retried, so the guard redirects at once. */
function retryUnlessUnauthorized(failureCount: number, error: Error): boolean {
  if (error instanceof ApiError && (error.status === 401 || error.status === 403)) return false;
  return failureCount < 1;
}

export const meQuery = queryOptions({
  queryKey: ME_KEY,
  queryFn: ({ signal }) => apiFetch(endpoints.me, { signal }),
  retry: retryUnlessUnauthorized,
});

export const sessionsQuery = queryOptions({
  queryKey: ["sessions"],
  queryFn: ({ signal }) => apiFetch(endpoints.sessions, { signal }),
});

export const passkeysQuery = queryOptions({
  queryKey: ["passkeys"],
  queryFn: ({ signal }) => apiFetch(endpoints.passkeys, { signal }),
});

export const invitesQuery = queryOptions({
  queryKey: ["invites"],
  queryFn: ({ signal }) => apiFetch(endpoints.invites, { signal }),
});

export const desktopsQuery = queryOptions({
  queryKey: ["desktops"],
  queryFn: ({ signal }) => apiFetch(endpoints.desktops, { signal }),
});

// --- Ceremony results ---

/** The library's response interfaces have no index signature; the API schema is an open object. */
function wireResponse<T extends { id: string; type: "public-key" }>(response: T): T & Record<string, unknown> {
  return { ...response };
}

export type Cancelled = { status: "cancelled" };

/** Thrown by `withFreshAuth` when the user dismisses the re-authentication prompt. */
export class ReauthCancelledError extends Error {
  constructor() {
    super("Re-authentication was cancelled");
    this.name = "ReauthCancelledError";
  }
}

// --- Fresh re-auth ---

/** Runs the passkey re-auth ceremony (`/api/auth/reauth/*`), opening the 5 minute fresh window. */
export async function reauth(): Promise<{ status: "ok"; reauthAt: number } | Cancelled> {
  const { challengeId, options } = await apiFetch(endpoints.reauthOptions);
  const result = await getPasskey(options);
  if (result.status === "cancelled") return result;
  const { reauthAt } = await apiFetch(endpoints.reauthVerify, { body: { challengeId, response: wireResponse(result.response) } });
  return { status: "ok", reauthAt };
}

/**
 * Runs `fn`; when the server answers `fresh_required`, runs the re-auth ceremony once and retries
 * `fn` once. A cancelled ceremony throws `ReauthCancelledError`. `reauthFn` is injectable for tests.
 */
export async function withFreshAuth<T>(fn: () => Promise<T>, reauthFn: typeof reauth = reauth): Promise<T> {
  try {
    return await fn();
  } catch (error) {
    if (!(error instanceof ApiError) || error.code !== "fresh_required") throw error;
  }
  const result = await reauthFn();
  if (result.status === "cancelled") throw new ReauthCancelledError();
  return fn();
}

// --- Auth flows ---

/**
 * Drops every cached value (`me`, passkeys, sessions, invites, desktops and the desktop link data).
 * Run on sign-in, register, recover and when a call answers 401, so one account never sees another's
 * cached data in the same tab (security review L2, N5).
 */
export function clearAccountCache(queryClient: QueryClient): void {
  queryClient.clear();
  // A New session draft and the text typed in a chat belong to the account that typed it.
  resetNewSessionDrafts();
  resetChatDrafts();
  // The offline snapshots of the previous session must not outlive it (features/offline/accountCache.ts).
  void forgetAccountSnapshots();
}

export interface RegisterInput {
  invite: string;
  handle: string;
  displayName: string;
}

/** Register options, passkey creation, verify. `recoveryCodes` (10) are shown once. */
export async function registerAccount(
  queryClient: QueryClient,
  input: RegisterInput,
): Promise<{ status: "ok"; user: User; recoveryCodes: string[] } | Cancelled> {
  const { challengeId, options } = await apiFetch(endpoints.registerOptions, { body: input });
  const result = await createPasskey(options);
  if (result.status === "cancelled") return result;
  const verified = await apiFetch(endpoints.registerVerify, { body: { challengeId, response: wireResponse(result.response) } });
  clearAccountCache(queryClient);
  setRecoveryBatch(verified.batch);
  return { status: "ok", user: verified.user, recoveryCodes: verified.recoveryCodes };
}

/** Usernameless sign-in: options without a handle, assertion, verify. */
export async function signIn(queryClient: QueryClient): Promise<{ status: "ok"; user: User } | Cancelled> {
  const { challengeId, options } = await apiFetch(endpoints.loginOptions);
  const result = await getPasskey(options);
  if (result.status === "cancelled") return result;
  const verified = await apiFetch(endpoints.loginVerify, { body: { challengeId, response: wireResponse(result.response) } });
  clearAccountCache(queryClient);
  return { status: "ok", user: verified.user };
}

/** Consumes a recovery code. The session that follows is `recoveryOnly` (adds a passkey only). */
export async function recoverAccount(queryClient: QueryClient, input: { handle: string; code: string }) {
  const result = await apiFetch(endpoints.recover, { body: input });
  clearAccountCache(queryClient);
  return result;
}

/** Ends the session and drops every cached account value. */
export async function signOut(queryClient: QueryClient, options: { keepLowCodes?: boolean } = {}): Promise<void> {
  // The reminder belongs to the account that recovered; it survives only the recover flow's own sign-out.
  if (!options.keepLowCodes) clearLowCodes();
  // The subscription belongs to this browser and this account: remove it while the session still works.
  // A failure here must not keep the user signed in.
  await unsubscribeFromPush().catch(() => undefined);
  // The server also deletes this device's push subscriptions with the session, in case the call above failed.
  const device = await getDeviceKeyInfo().catch(() => null);
  try {
    await apiFetch(endpoints.logout, device?.deviceId ? { body: { deviceId: device.deviceId } } : {});
  } finally {
    queryClient.clear();
    resetNewSessionDrafts();
    resetChatDrafts();
    // The encrypted content cache, the snapshots and any push subscription left behind go with the session.
    await releaseDevice();
  }
}

// --- Account calls ---

export async function updateDisplayName(queryClient: QueryClient, displayName: string): Promise<User> {
  const { user } = await apiFetch(endpoints.patchMe, { body: { displayName } });
  queryClient.setQueryData<Me>(ME_KEY, (me) => (me ? { ...me, user } : me));
  return user;
}

export async function revokeSession(queryClient: QueryClient, id: string): Promise<void> {
  await apiFetch(endpoints.revokeSession, { params: { id } });
  await queryClient.invalidateQueries({ queryKey: sessionsQuery.queryKey });
}

/** Adds a passkey (fresh, or recoveryOnly session). Returns `cancelled` when the prompt is dismissed. */
export async function addPasskey(queryClient: QueryClient, name?: string) {
  let cancelled = false;
  const added = await withFreshAuth(async () => {
    const { challengeId, options } = await apiFetch(endpoints.addPasskeyOptions);
    const result = await createPasskey(options);
    if (result.status === "cancelled") {
      cancelled = true;
      return null;
    }
    return apiFetch(endpoints.addPasskeyVerify, { body: { challengeId, response: wireResponse(result.response), name } });
  });
  if (cancelled || !added) return { status: "cancelled" } as const;
  await queryClient.invalidateQueries({ queryKey: passkeysQuery.queryKey });
  await queryClient.invalidateQueries({ queryKey: ME_KEY });
  return { status: "ok", passkey: added.passkey } as const;
}

export async function renamePasskey(queryClient: QueryClient, id: string, name: string) {
  const { passkey } = await withFreshAuth(() => apiFetch(endpoints.renamePasskey, { params: { id }, body: { name } }));
  await queryClient.invalidateQueries({ queryKey: passkeysQuery.queryKey });
  return passkey;
}

/** Fails with `last_passkey` for the only passkey. */
export async function deletePasskey(queryClient: QueryClient, id: string): Promise<void> {
  await withFreshAuth(() => apiFetch(endpoints.deletePasskey, { params: { id } }));
  await queryClient.invalidateQueries({ queryKey: passkeysQuery.queryKey });
}

/**
 * New recovery codes, shown once; every older code stops working. The Worker clears `recoveryAckAt`, so `me` is NOT
 * refreshed here (the guard would swap the screen and drop the codes before they are saved): the caller calls
 * `acknowledgeRecoveryCodes` when the user confirms.
 */
export async function regenerateRecoveryCodes(_queryClient: QueryClient): Promise<string[]> {
  const { recoveryCodes, batch } = await withFreshAuth(() => apiFetch(endpoints.regenerateRecoveryCodes));
  setRecoveryBatch(batch);
  clearLowCodes();
  return recoveryCodes;
}

/** "I saved these": records the acknowledgement on the server (`recoveryAckAt`) and refreshes `me`. */
export async function acknowledgeRecoveryCodes(queryClient: QueryClient): Promise<void> {
  const batch = getRecoveryBatch();
  if (batch === null) throw new Error("No recovery codes to acknowledge");
  await apiFetch(endpoints.ackRecoveryCodes, { body: { batch } });
  await queryClient.invalidateQueries({ queryKey: ME_KEY });
}

/** Fails with `invites_paused` or `quota_exceeded`. `code` is shown once. */
export async function createInvite(queryClient: QueryClient) {
  const created = await apiFetch(endpoints.createInvite);
  await queryClient.invalidateQueries({ queryKey: invitesQuery.queryKey });
  await queryClient.invalidateQueries({ queryKey: ME_KEY });
  return created;
}

export async function revokeInvite(queryClient: QueryClient, id: string): Promise<void> {
  await apiFetch(endpoints.revokeInvite, { params: { id } });
  await queryClient.invalidateQueries({ queryKey: invitesQuery.queryKey });
  await queryClient.invalidateQueries({ queryKey: ME_KEY });
}

export type PushPrefs = ReturnType<typeof PushPrefsResponse.parse>;

/** Per-desktop notification preferences (W-11). The contract has no GET: keep the PUT result. */
export function savePushPrefs(desktopId: string, prefs: PushPrefs): Promise<PushPrefs> {
  return apiFetch(endpoints.putPushPrefs, { params: { desktopId }, body: prefs });
}
