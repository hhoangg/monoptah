// Route of /admin. Owner: WS-S (workstreams.md 3.1). Needs role admin (AdminGuard) and a fresh session.
import { createFileRoute } from "@tanstack/react-router";
import { AdminGuard } from "@/features/admin/AdminGuard";
import { AdminHome } from "@/features/admin/AdminHome";

export const Route = createFileRoute("/admin/")({
  component: Page,
});

function Page() {
  return <AdminGuard>{() => <AdminHome />}</AdminGuard>;
}
