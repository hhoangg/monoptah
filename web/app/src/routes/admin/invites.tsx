// Route stub of /admin/invites. Owner: WS-S (workstreams.md 3.1).
import { createFileRoute } from "@tanstack/react-router";
import { RouteStub } from "@/ui/RouteStub";

export const Route = createFileRoute("/admin/invites")({
  component: Page,
});

function Page() {
  const params = Route.useParams();
  return <RouteStub route="/admin/invites" purpose="Admin: create and revoke invite codes." params={params} />;
}
