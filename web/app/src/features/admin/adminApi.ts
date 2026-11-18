// The admin API (A-09) as typed endpoints, query options and write helpers. Every admin route needs
// role admin and a fresh session. Reads are plain calls: a stale session answers 403 `fresh_required`,
// which the screens turn into an explicit "Confirm it's you" step instead of an unexpected prompt.
// Writes go through `withFreshAuth`: the user just clicked, so the passkey prompt is expected.
import { infiniteQueryOptions, queryOptions, type QueryClient } from "@tanstack/react-query";
import {
  AdminAuditResponse,
  AdminBlockBody,
  AdminBlockResponse,
  AdminGrantInvitesBody,
  AdminPutSettingsBody,
  AdminSettingsSchema,
  AdminUsersResponse,
  HealthResponse,
} from "@monoptah/protocol";
import { ApiError, apiFetch, endpoint } from "@/features/accounts/api";
import { invitesQuery, ME_KEY, withFreshAuth } from "@/features/accounts/queries";

export type AdminUser = ReturnType<typeof AdminUsersResponse.parse>["users"][number];
export type AdminSettings = ReturnType<typeof AdminSettingsSchema.parse>;
export type AdminAuditEvent = ReturnType<typeof AdminAuditResponse.parse>["events"][number];

export const adminEndpoints = {
  users: endpoint("GET", "/api/admin/users", AdminUsersResponse),
  block: endpoint("POST", "/api/admin/users/:id/block", AdminBlockResponse, AdminBlockBody),
  unblock: endpoint("POST", "/api/admin/users/:id/unblock", HealthResponse),
  grantInvites: endpoint("POST", "/api/admin/users/:id/invites", HealthResponse, AdminGrantInvitesBody),
  settings: endpoint("GET", "/api/admin/settings", AdminSettingsSchema),
  putSettings: endpoint("PUT", "/api/admin/settings", AdminSettingsSchema, AdminPutSettingsBody),
  audit: endpoint("GET", "/api/admin/audit", AdminAuditResponse),
} as const;

/** The longest reason the contract accepts. */
export const MAX_REASON_LENGTH = 500;
/** The most invites one grant can add. */
export const MAX_GRANT = 100;
export const AUDIT_PAGE_SIZE = 10;

/** 401, 403 and 409-style answers are final: retrying cannot change them. */
function retryTransient(failureCount: number, error: Error): boolean {
  if (error instanceof ApiError && error.status >= 400 && error.status < 500) return false;
  return failureCount < 1;
}

export const ADMIN_KEY = ["admin"] as const;

export const adminSettingsQuery = queryOptions({
  queryKey: [...ADMIN_KEY, "settings"],
  queryFn: ({ signal }) => apiFetch(adminEndpoints.settings, { signal }),
  retry: retryTransient,
});

export const adminUsersQuery = queryOptions({
  queryKey: [...ADMIN_KEY, "users"],
  queryFn: ({ signal }) => apiFetch(adminEndpoints.users, { signal }),
  retry: retryTransient,
});

export const adminAuditQuery = infiniteQueryOptions({
  queryKey: [...ADMIN_KEY, "audit"],
  queryFn: ({ pageParam, signal }) => apiFetch(adminEndpoints.audit, { query: { limit: AUDIT_PAGE_SIZE, cursor: pageParam }, signal }),
  initialPageParam: undefined as string | undefined,
  getNextPageParam: (last) => last.nextCursor ?? undefined,
  retry: retryTransient,
});

/** True when the error means "this session is not fresh yet". */
export function needsFreshSession(error: unknown): boolean {
  return error instanceof ApiError && error.code === "fresh_required";
}

/** True when the server says this account is not an admin. */
export function isForbidden(error: unknown): boolean {
  return error instanceof ApiError && error.code === "forbidden";
}

async function refreshAdminData(queryClient: QueryClient): Promise<void> {
  await queryClient.invalidateQueries({ queryKey: ADMIN_KEY });
}

/** The "Pause invites" switch. Audited by the server; returns the new settings. */
export async function setInvitesPaused(queryClient: QueryClient, invitesPaused: boolean): Promise<AdminSettings> {
  const settings = await withFreshAuth(() => apiFetch(adminEndpoints.putSettings, { body: { invitesPaused } }));
  queryClient.setQueryData(adminSettingsQuery.queryKey, settings);
  await queryClient.invalidateQueries({ queryKey: adminAuditQuery.queryKey });
  return settings;
}

/** Blocks one user, or with `cascade` the user and everyone they invited. Returns how many users changed. */
export async function blockUser(queryClient: QueryClient, id: string, input: { cascade: boolean; reason: string }): Promise<number> {
  const { blocked } = await withFreshAuth(() => apiFetch(adminEndpoints.block, { params: { id }, body: input }));
  await refreshAdminData(queryClient);
  return blocked;
}

/** Restores sign-in for this user only. Descendants stay blocked; revoked tokens and grants stay revoked. */
export async function unblockUser(queryClient: QueryClient, id: string): Promise<void> {
  await withFreshAuth(() => apiFetch(adminEndpoints.unblock, { params: { id } }));
  await refreshAdminData(queryClient);
}

export async function grantInvites(queryClient: QueryClient, id: string, count: number): Promise<void> {
  await withFreshAuth(() => apiFetch(adminEndpoints.grantInvites, { params: { id }, body: { count } }));
  await refreshAdminData(queryClient);
  // The admin may have granted invites to themselves.
  await queryClient.invalidateQueries({ queryKey: invitesQuery.queryKey });
  await queryClient.invalidateQueries({ queryKey: ME_KEY });
}
