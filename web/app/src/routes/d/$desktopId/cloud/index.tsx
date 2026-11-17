// Route stub of /d/:desktopId/cloud. Owner: WS-C (workstreams.md 3.1).
import { createFileRoute } from "@tanstack/react-router";
import { RouteStub } from "@/ui/RouteStub";

export const Route = createFileRoute("/d/$desktopId/cloud/")({
  component: Page,
});

function Page() {
  const params = Route.useParams();
  return <RouteStub route="/d/:desktopId/cloud" purpose="Cloud instance list: state, sessions per instance, Stop and Resume." params={params} />;
}
