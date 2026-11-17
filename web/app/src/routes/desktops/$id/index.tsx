// Route stub of /desktops/:id. Owner: WS-M (workstreams.md 3.1).
import { createFileRoute } from "@tanstack/react-router";
import { RouteStub } from "@/ui/RouteStub";

export const Route = createFileRoute("/desktops/$id/")({
  component: Page,
});

function Page() {
  const params = Route.useParams();
  return <RouteStub route="/desktops/:id" purpose="Manage one desktop: rename, color, devices and grants, revoke." params={params} />;
}
