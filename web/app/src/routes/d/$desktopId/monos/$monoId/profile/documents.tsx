// Route stub of /d/:desktopId/monos/:monoId/profile/documents. Owner: WS-P (workstreams.md 3.1).
import { createFileRoute } from "@tanstack/react-router";
import { RouteStub } from "@/ui/RouteStub";

export const Route = createFileRoute("/d/$desktopId/monos/$monoId/profile/documents")({
  component: Page,
});

function Page() {
  const params = Route.useParams();
  return <RouteStub route="/d/:desktopId/monos/:monoId/profile/documents" purpose="Documents of the Mono profile." params={params} />;
}
