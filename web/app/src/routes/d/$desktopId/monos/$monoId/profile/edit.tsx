// Route stub of /d/:desktopId/monos/:monoId/profile/edit. Owner: WS-P (workstreams.md 3.1).
import { createFileRoute } from "@tanstack/react-router";
import { RouteStub } from "@/ui/RouteStub";

export const Route = createFileRoute("/d/$desktopId/monos/$monoId/profile/edit")({
  component: Page,
});

function Page() {
  const params = Route.useParams();
  return <RouteStub route="/d/:desktopId/monos/:monoId/profile/edit" purpose="Edit the Mono profile." params={params} />;
}
