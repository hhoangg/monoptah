// Route stub of /d/:desktopId/s/:sessionId. Owner: WS-T (workstreams.md 3.1).
import { createFileRoute } from "@tanstack/react-router";
import { RouteStub } from "@/ui/RouteStub";

export const Route = createFileRoute("/d/$desktopId/s/$sessionId")({
  component: Page,
});

function Page() {
  const params = Route.useParams();
  return <RouteStub route="/d/:desktopId/s/:sessionId" purpose="Transcript and composer for one session." params={params} />;
}
