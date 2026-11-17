// Route stub of /admin. Owner: WS-S (workstreams.md 3.1).
import { createFileRoute } from "@tanstack/react-router";
import { RouteStub } from "@/ui/RouteStub";

export const Route = createFileRoute("/admin/")({
  component: Page,
});

function Page() {
  const params = Route.useParams();
  return <RouteStub route="/admin" purpose="Minimal admin overview (role admin, fresh sign-in)." params={params} />;
}
