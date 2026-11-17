// Route stub of /d/:desktopId/settings. Owner: WS-S (workstreams.md 3.1).
import { createFileRoute } from "@tanstack/react-router";
import { RouteStub } from "@/ui/RouteStub";

export const Route = createFileRoute("/d/$desktopId/settings/")({
  component: Page,
});

function Page() {
  const params = Route.useParams();
  return <RouteStub route="/d/:desktopId/settings" purpose="Per-desktop settings: model defaults, project selection, notification categories, Reconnect." params={params} />;
}
