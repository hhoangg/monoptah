// Route stub of /. Owner: WS-L (workstreams.md 3.1).
import { createFileRoute } from "@tanstack/react-router";
import { RouteStub } from "@/ui/RouteStub";

export const Route = createFileRoute("/")({
  component: Page,
});

function Page() {
  const params = Route.useParams();
  return <RouteStub route="/" purpose="Restores the last place (last used desktop, project, session) and redirects there, else /desktops." params={params} />;
}
