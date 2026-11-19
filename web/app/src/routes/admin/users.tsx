// Route of /admin/users. Owner: WS-S (workstreams.md 3.1). Needs role admin (AdminGuard) and a fresh session.
// `?user=<id>` selects the user shown in the detail panel (a full-screen detail on mobile).
import { createFileRoute } from "@tanstack/react-router";
import { AdminGuard } from "@/features/admin/AdminGuard";
import { AdminUsers } from "@/features/admin/AdminUsers";

export const Route = createFileRoute("/admin/users")({
  validateSearch: (search: Record<string, unknown>): { user?: string } => (typeof search.user === "string" && search.user !== "" ? { user: search.user } : {}),
  component: Page,
});

function Page() {
  const { user } = Route.useSearch();
  const navigate = Route.useNavigate();
  return (
    <AdminGuard>
      {(me) => <AdminUsers selfId={me.id} selectedId={user} onSelect={(id) => void navigate({ search: id ? { user: id } : {} })} />}
    </AdminGuard>
  );
}
