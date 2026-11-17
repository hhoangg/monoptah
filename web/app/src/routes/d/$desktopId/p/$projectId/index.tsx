// Route stub of /d/:desktopId/p/:projectId. Owner: WS-T (workstreams.md 3.1).
import { createFileRoute } from "@tanstack/react-router";
import { RouteStub } from "@/ui/RouteStub";

export const Route = createFileRoute("/d/$desktopId/p/$projectId/")({
  component: Page,
});

function Page() {
  const params = Route.useParams();
  return <RouteStub route="/d/:desktopId/p/:projectId" purpose="Project: the sessions list." params={params} />;
}
