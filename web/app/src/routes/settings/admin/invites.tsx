// Route of /settings/admin/invites: the invite tree, a page of the settings shell. Needs role admin (AdminGuard) and a
// fresh session.
import { createFileRoute } from "@tanstack/react-router";
import { AdminGuard } from "@/features/admin/AdminGuard";
import { AdminInvites } from "@/features/admin/AdminInvites";

export const Route = createFileRoute("/settings/admin/invites")({
  component: Page,
});

function Page() {
  return <AdminGuard>{() => <AdminInvites />}</AdminGuard>;
}
