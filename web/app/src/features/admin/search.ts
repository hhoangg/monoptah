// The search params of the admin users page: `?user=<id>` selects the user shown in the detail panel (a full-screen
// detail below 640px). Shared by /settings/admin/users and the old /admin/users URL, which redirects with it.
export interface AdminUsersSearch {
  user?: string;
}

export function validateAdminUsersSearch(search: Record<string, unknown>): AdminUsersSearch {
  return typeof search.user === "string" && search.user !== "" ? { user: search.user } : {};
}
