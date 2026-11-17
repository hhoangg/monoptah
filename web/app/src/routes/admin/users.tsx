// Route stub of /admin/users. Owner: WS-S (workstreams.md 3.1).
import { createFileRoute } from "@tanstack/react-router";
import { RouteStub } from "@/ui/RouteStub";

export const Route = createFileRoute("/admin/users")({
  component: Page,
});

function Page() {
  const params = Route.useParams();
  return <RouteStub route="/admin/users" purpose="Admin: user list, block and unblock." params={params} />;
}
