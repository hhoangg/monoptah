// Route stub of /d/:desktopId. Owner: WS-T (workstreams.md 3.1).
import { createFileRoute } from "@tanstack/react-router";
import { RouteStub } from "@/ui/RouteStub";

export const Route = createFileRoute("/d/$desktopId/")({
  component: Page,
});

function Page() {
  const params = Route.useParams();
  return <RouteStub route="/d/:desktopId" purpose="Home for one desktop: projects and recent sessions (catalog)." params={params} />;
}
