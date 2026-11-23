// Route of /settings/admin: the admin overview, a page of the settings shell (routes/settings/route.tsx). Needs role
// admin (AdminGuard) and a fresh session.
import { createFileRoute } from "@tanstack/react-router";
import { AdminGuard } from "@/features/admin/AdminGuard";
import { AdminHome } from "@/features/admin/AdminHome";

export const Route = createFileRoute("/settings/admin/")({
  component: Page,
});

function Page() {
  return <AdminGuard>{() => <AdminHome />}</AdminGuard>;
}
