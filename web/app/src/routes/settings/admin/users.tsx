// Route of /settings/admin/users: the users list and one user's detail, a page of the settings shell. Needs role admin
// (AdminGuard) and a fresh session. `?user=<id>` selects the user shown in the detail panel (a full-screen detail below
// 640px).
import { createFileRoute } from "@tanstack/react-router";
import { AdminGuard } from "@/features/admin/AdminGuard";
import { AdminUsers } from "@/features/admin/AdminUsers";
import { validateAdminUsersSearch } from "@/features/admin/search";

export const Route = createFileRoute("/settings/admin/users")({
  validateSearch: validateAdminUsersSearch,
  component: Page,
});

function Page() {
  const { user } = Route.useSearch();
  const navigate = Route.useNavigate();
  return (
    <AdminGuard>
      {(me) => <AdminUsers selfId={me.id} selectedId={user} onSelect={(id, options) => void navigate({ search: id ? { user: id } : {}, ...options })} />}
    </AdminGuard>
  );
}
