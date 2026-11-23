// Desktops, devices and grants API of the web app: typed endpoints, query hooks and mutation helpers.
//
// Owner: WS-M (U-01, W-04). Screens read with `useDesktops`, `useDesktop`, `useDesktopGrants` and
// `useDevices`, and write with the plain async helpers below (`renameDesktop`, `revokeGrant`, ...).
// The helpers take the query client so they also work outside React (the link-state screens and the
// switcher of Task 2 call `requestGrant` directly). Revoking a desktop or a grant is a fresh route:
// both helpers retry once after the passkey re-auth ceremony (`withFreshAuth`).
import {
  DesktopDirectoryResponse,
  DesktopsResponse,
  DevicesResponse,
  GrantResponse,
  GrantsResponse,
  HealthResponse,
  PatchDesktopBody,
  PatchDesktopResponse,
  PutGrantBody,
  RequestGrantBody,
  SelectionTicketResponse,
  TerminalTicketBody,
  TerminalTicketResponse,
} from "@monoptah/protocol";
import { queryOptions, useQuery, type QueryClient } from "@tanstack/react-query";
import { apiFetch, endpoint, isApiError } from "@/features/accounts/api";
import { withFreshAuth } from "@/features/accounts/queries";
import { withRateLimitRetry } from "@/lib/rateLimit";
import { clearPersistedCache } from "@/data/persist";
import { forgetDesktopLocally } from "@/features/settings/desktopLink";
import { getLinkConnection, resetLinkConnections } from "@/link";
import { forgetDeviceKey, getDeviceKeyInfo } from "@/link/keys";
import { forgetPins } from "@/link/pins";

type Output<S extends { parse(data: unknown): unknown }> = ReturnType<S["parse"]>;

export type DesktopSummary = Output<typeof DesktopsResponse>["desktops"][number];
export type DesktopDirectory = Output<typeof DesktopDirectoryResponse>;
export type DeviceSummary = Output<typeof DevicesResponse>["devices"][number];
export type Grant = Output<typeof GrantResponse>["grant"];
export type Permission = NonNullable<Grant["permission"]>;

const Ok = HealthResponse;

export const desktopEndpoints = {
  list: endpoint("GET", "/api/desktops", DesktopsResponse),
  get: endpoint("GET", "/api/desktops/:id", DesktopDirectoryResponse),
  patch: endpoint("PATCH", "/api/desktops/:id", PatchDesktopResponse, PatchDesktopBody),
  revoke: endpoint("DELETE", "/api/desktops/:id", Ok),
  devices: endpoint("GET", "/api/devices", DevicesResponse),
  forgetDevice: endpoint("DELETE", "/api/devices/:id", Ok),
  grants: endpoint("GET", "/api/desktops/:id/grants", GrantsResponse),
  requestGrant: endpoint("POST", "/api/desktops/:id/grants", GrantResponse, RequestGrantBody),
  putGrant: endpoint("PUT", "/api/desktops/:id/grants/:deviceId", GrantResponse, PutGrantBody),
  selectionTicket: endpoint("POST", "/api/desktops/:id/grants/:deviceId/selection-ticket", SelectionTicketResponse),
  terminalTicket: endpoint("POST", "/api/desktops/:id/grants/:deviceId/terminal-ticket", TerminalTicketResponse, TerminalTicketBody),
};

// --- Query keys ---

/** Every key is under `["desktops"]`, so one invalidation of the root refreshes everything. */
export const desktopKeys = {
  /** The list. Same key as `desktopsQuery` of the account screens, so the cache is shared. */
  list: ["desktops"] as const,
  detail: (id: string) => ["desktops", "detail", id] as const,
  grantsRoot: ["desktops", "grants"] as const,
  grants: (id: string) => ["desktops", "grants", id] as const,
  devices: ["desktops", "devices"] as const,
  thisDevice: ["desktops", "this-device"] as const,
};

/** How often the lists refetch while the page is visible, so the online badge follows presence (Worker caches it 10 s). */
export const DESKTOPS_REFRESH_MS = 15_000;

function visibleEvery(ms: number): () => number | false {
  return () => (typeof document === "undefined" || document.visibilityState === "visible" ? ms : false);
}

const whileVisible = visibleEvery(DESKTOPS_REFRESH_MS);

// --- Queries ---

export const desktopsListQuery = queryOptions({
  queryKey: desktopKeys.list,
  queryFn: ({ signal }) => apiFetch(desktopEndpoints.list, { signal }),
  refetchInterval: whileVisible,
});

/** A missing or foreign desktop is a final answer: no retry, so the not-found state shows at once. */
const retryUnlessMissing = withRateLimitRetry((failureCount, error) => {
  if (isApiError(error, "not_found") || isApiError(error, "forbidden")) return false;
  return failureCount < 1;
});

/**
 * The directory entry of one desktop: what the list has plus the public key. It is read once (the default stale time
 * applies) and not polled: what changes while a page is open (name, color, avatar, presence, pending count) comes from
 * the list, which is polled once for the whole app. Mutations invalidate it.
 */
export const desktopDetailQuery = (id: string) =>
  queryOptions({
    queryKey: desktopKeys.detail(id),
    queryFn: ({ signal }) => apiFetch(desktopEndpoints.get, { params: { id }, signal }),
    retry: retryUnlessMissing,
  });

/** How often the grants a screen only glances at (the cues of the rail and the switcher) refetch while the page is visible. */
export const GRANTS_GLANCE_STALE_MS = 60_000;

export interface GrantsQueryOptions {
  /**
   * Refetch every 15 s while visible. Only the screens that wait for a change (the access card of the desktop page,
   * the composer that asked for control) do; a glance (the rail, the switcher) refetches every GRANTS_GLANCE_STALE_MS
   * while visible (and is fresh for that long), instead of once per desktop every 15 s.
   */
  poll?: boolean;
}

export const desktopGrantsQuery = (id: string, { poll = true }: GrantsQueryOptions = {}) =>
  queryOptions({
    queryKey: desktopKeys.grants(id),
    queryFn: ({ signal }) => apiFetch(desktopEndpoints.grants, { params: { id }, signal }),
    ...(poll ? { refetchInterval: whileVisible } : { staleTime: GRANTS_GLANCE_STALE_MS, refetchInterval: visibleEvery(GRANTS_GLANCE_STALE_MS) }),
  });

export const devicesQuery = queryOptions({
  queryKey: desktopKeys.devices,
  queryFn: ({ signal }) => apiFetch(desktopEndpoints.devices, { signal }),
});

export function useDesktops() {
  return useQuery(desktopsListQuery);
}

export function useDesktop(id: string) {
  return useQuery(desktopDetailQuery(id));
}

export function useDesktopGrants(id: string, options: GrantsQueryOptions & { enabled?: boolean } = {}) {
  const { enabled, ...query } = options;
  return useQuery({ ...desktopGrantsQuery(id, query), ...(enabled === undefined ? {} : { enabled }) });
}

export function useDevices() {
  return useQuery(devicesQuery);
}

// --- Mutations ---

async function refreshDesktop(queryClient: QueryClient, id: string): Promise<void> {
  await Promise.all([
    queryClient.invalidateQueries({ queryKey: desktopKeys.list, exact: true }),
    queryClient.invalidateQueries({ queryKey: desktopKeys.detail(id) }),
  ]);
}

/** Renames a desktop. The server trims and bounds the name (1..64 chars). */
export async function renameDesktop(queryClient: QueryClient, id: string, name: string): Promise<DesktopSummary> {
  const { desktop } = await apiFetch(desktopEndpoints.patch, { params: { id }, body: { name } });
  await refreshDesktop(queryClient, id);
  return desktop;
}

/** Sets the desktop color (`null` clears it). The value is one of the palette values of `colors.ts`. */
export async function setDesktopColor(queryClient: QueryClient, id: string, color: string | null): Promise<DesktopSummary> {
  const { desktop } = await apiFetch(desktopEndpoints.patch, { params: { id }, body: { color } });
  await refreshDesktop(queryClient, id);
  return desktop;
}

/** What a client sends to set the avatar: the emoji, or the base64 of a re-encoded image. Null goes back to initials. */
export type AvatarChoice = { kind: "emoji"; emoji: string } | { kind: "image"; mime: "image/png" | "image/jpeg" | "image/webp"; data: string } | null;

/** Sets (or clears, with null) the avatar of a desktop, for every web device of the account. */
export async function setDesktopAvatar(queryClient: QueryClient, id: string, avatar: AvatarChoice): Promise<DesktopSummary> {
  const { desktop } = await apiFetch(desktopEndpoints.patch, { params: { id }, body: { avatar } });
  await refreshDesktop(queryClient, id);
  return desktop;
}

export interface RevokeDesktopOptions {
  /**
   * Runs after the server revoked the desktop and before its queries are removed. The detail screen
   * navigates away here, so the removal never makes its own observer refetch a deleted desktop.
   */
  afterRevoke?: () => Promise<unknown> | void;
}

/**
 * Revokes a desktop (fresh route: re-authenticates once when the server asks). After the server
 * succeeded it also drops what this browser holds of that desktop: the live link, the persisted
 * (encrypted) cache and every query of it.
 */
export async function revokeDesktop(queryClient: QueryClient, id: string, options: RevokeDesktopOptions = {}): Promise<void> {
  await withFreshAuth(() => apiFetch(desktopEndpoints.revoke, { params: { id } }));
  try {
    getLinkConnection(id).stop();
  } catch {
    // A connection that cannot stop must not keep the cache around.
  }
  await clearPersistedCache(id);
  // The same wipe as "remove this desktop from this device": the per-desktop records of the other features
  // (last place, Mono seen map, reactions) go too.
  try {
    await forgetDesktopLocally(id);
  } catch {
    // One handler failing must not keep the revoked desktop's screen from closing.
  }
  await options.afterRevoke?.();
  queryClient.removeQueries({ queryKey: desktopKeys.detail(id) });
  queryClient.removeQueries({ queryKey: desktopKeys.grants(id) });
  queryClient.removeQueries({ queryKey: ["link", id] });
  await queryClient.invalidateQueries({ queryKey: desktopKeys.list, exact: true });
}

/**
 * Asks for access of one device to one desktop (`POST /api/desktops/:id/grants`). Idempotent: an
 * existing pending or confirmed grant comes back unchanged. A revoked grant answers 409 `conflict`
 * (an `ApiError`), which the caller shows as the revoked screen. Plain async: usable outside React.
 */
export async function requestGrant(desktopId: string, deviceId: string): Promise<Grant> {
  const { grant } = await apiFetch(desktopEndpoints.requestGrant, { params: { id: desktopId }, body: { deviceId } });
  return grant;
}

/**
 * `requestGrant` plus a refresh of the grants and the pending count of that desktop. The request is idempotent and the
 * link repeats it on every connect, so a grant that the cache already holds unchanged costs no refetch; the list only
 * gets marked stale (its 15 s poll and the next mount pick the pending count up).
 */
export async function requestGrantAndRefresh(queryClient: QueryClient, desktopId: string, deviceId: string): Promise<Grant> {
  const grant = await requestGrant(desktopId, deviceId);
  const known = queryClient.getQueryData<{ grants: Grant[] }>(desktopKeys.grants(desktopId))?.grants.find((item) => item.deviceId === grant.deviceId);
  const unchanged = known?.state === grant.state && known.permission === grant.permission && known.requestedPermission === grant.requestedPermission;
  await Promise.all([
    unchanged ? undefined : queryClient.invalidateQueries({ queryKey: desktopKeys.grants(desktopId) }, { cancelRefetch: false }),
    queryClient.invalidateQueries({ queryKey: desktopKeys.list, exact: true, refetchType: "none" }),
  ]);
  return grant;
}

/**
 * Asks for another permission. Lowering to "read" is applied by the desktop at once; raising to
 * "control" waits for the person at the desktop to confirm (R-15). The server never writes `permission`.
 */
export async function setRequestedPermission(queryClient: QueryClient, desktopId: string, deviceId: string, requested: Permission): Promise<Grant> {
  const { grant } = await apiFetch(desktopEndpoints.putGrant, { params: { id: desktopId, deviceId }, body: { requestedPermission: requested } });
  await queryClient.invalidateQueries({ queryKey: desktopKeys.grants(desktopId) });
  return grant;
}

/** Revokes one device's grant on a desktop (fresh route). */
export async function revokeGrant(queryClient: QueryClient, desktopId: string, deviceId: string): Promise<Grant> {
  const { grant } = await withFreshAuth(() => apiFetch(desktopEndpoints.putGrant, { params: { id: desktopId, deviceId }, body: { state: "revoked" } }));
  await Promise.all([
    queryClient.invalidateQueries({ queryKey: desktopKeys.grants(desktopId) }),
    queryClient.invalidateQueries({ queryKey: desktopKeys.list, exact: true }),
  ]);
  return grant;
}

/**
 * A single-use ticket that proves a fresh passkey re-auth to the desktop, so this browser (a Control device) can widen
 * its selection there. Fresh route: prompts for the passkey once when the server asks. The ticket is bound to this
 * desktop and device and lives about two minutes; it is never stored or logged. 403 `forbidden` means the grant is not
 * Control (`ApiError`).
 */
export async function requestSelectionTicket(desktopId: string, deviceId: string): Promise<string> {
  const { ticket } = await withFreshAuth(() => apiFetch(desktopEndpoints.selectionTicket, { params: { id: desktopId, deviceId } }));
  return ticket;
}

/**
 * A single-use ticket (60 s) that proves a fresh passkey re-auth to the desktop for opening a terminal in `projectId`.
 * Fresh route, like `requestSelectionTicket`: the passkey prompt shows when the server asks for it. The ticket is bound
 * to this user, desktop, device and project, travels to the desktop inside `terminal.open` and is never stored or logged.
 */
export async function requestTerminalTicket(desktopId: string, deviceId: string, projectId: string): Promise<string> {
  const { ticket } = await withFreshAuth(() => apiFetch(desktopEndpoints.terminalTicket, { params: { id: desktopId, deviceId }, body: { projectId } }));
  return ticket;
}

/** Every step runs even when an earlier one failed; the first failure is rethrown at the end. */
async function runAll(steps: Array<() => Promise<unknown> | unknown>): Promise<void> {
  const failures: unknown[] = [];
  for (const step of steps) {
    try {
      await step();
    } catch (error) {
      failures.push(error);
    }
  }
  if (failures.length > 0) throw failures[0];
}

/**
 * Forgets a device: the server revokes every grant of it and deletes its push subscriptions. When it
 * is this browser's own device, the local state goes too: the device key, the pins of this device, the persisted
 * (encrypted) cache of every desktop, the live link connections and the link queries. A 404 for this
 * browser's own device means an earlier attempt already deleted it on the server (and failed locally),
 * so the local cleanup still runs.
 */
export async function forgetDevice(queryClient: QueryClient, deviceId: string): Promise<{ wasThisDevice: boolean }> {
  const local = await getDeviceKeyInfo().catch(() => null);
  const wasThisDevice = local?.deviceId === deviceId;
  try {
    await withFreshAuth(() => apiFetch(desktopEndpoints.forgetDevice, { params: { id: deviceId } }));
  } catch (error) {
    if (!(wasThisDevice && isApiError(error, "not_found"))) throw error;
  }
  const cleanup = wasThisDevice
    ? runAll([
        () => forgetDeviceKey(),
        () => forgetPins(deviceId),
        () => clearPersistedCache(),
        () => resetLinkConnections(),
        () => queryClient.removeQueries({ queryKey: ["link"] }),
      ])
    : Promise.resolve();
  await cleanup.finally(() =>
    Promise.all([
      queryClient.invalidateQueries({ queryKey: desktopKeys.devices }),
      queryClient.invalidateQueries({ queryKey: desktopKeys.grantsRoot }),
      queryClient.invalidateQueries({ queryKey: desktopKeys.list, exact: true }),
      queryClient.invalidateQueries({ queryKey: desktopKeys.thisDevice }),
    ]),
  );
  return { wasThisDevice };
}
