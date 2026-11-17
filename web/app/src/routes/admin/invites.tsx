// Route of /admin/invites. Owner: WS-S (workstreams.md 3.1). Needs role admin (AdminGuard) and a fresh session.
import { createFileRoute } from "@tanstack/react-router";
import { AdminGuard } from "@/features/admin/AdminGuard";
import { AdminInvites } from "@/features/admin/AdminInvites";

export const Route = createFileRoute("/admin/invites")({
  component: Page,
});

function Page() {
  return <AdminGuard>{() => <AdminInvites />}</AdminGuard>;
}
