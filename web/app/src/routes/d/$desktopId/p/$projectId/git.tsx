// Route stub of /d/:desktopId/p/:projectId/git. Owner: WS-P (workstreams.md 3.1).
import { createFileRoute } from "@tanstack/react-router";
import { RouteStub } from "@/ui/RouteStub";

export const Route = createFileRoute("/d/$desktopId/p/$projectId/git")({
  component: Page,
});

function Page() {
  const params = Route.useParams();
  return <RouteStub route="/d/:desktopId/p/:projectId/git" purpose="Git status and actions." params={params} />;
}
