// Route of /admin/users (old URL, kept for bookmarks and links): the users page now lives in the settings shell. A
// selected user (`?user=<id>`) is kept.
import { createFileRoute, redirect } from "@tanstack/react-router";
import { validateAdminUsersSearch } from "@/features/admin/search";

export const Route = createFileRoute("/admin/users")({
  validateSearch: validateAdminUsersSearch,
  beforeLoad: ({ search }) => {
    throw redirect({ to: "/settings/admin/users", search, replace: true });
  },
});
