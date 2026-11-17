// Route stub of /d/:desktopId/p/:projectId/changes/*. Owner: WS-P (workstreams.md 3.1).
import { createFileRoute } from "@tanstack/react-router";
import { RouteStub } from "@/ui/RouteStub";

export const Route = createFileRoute("/d/$desktopId/p/$projectId/changes/$")({
  component: Page,
});

function Page() {
  const params = Route.useParams();
  return <RouteStub route="/d/:desktopId/p/:projectId/changes/*" purpose="Working-tree diff (the splat path selects a file)." params={params} />;
}
