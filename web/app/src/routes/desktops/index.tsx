// Route stub of /desktops. Owner: WS-M (workstreams.md 3.1).
import { createFileRoute } from "@tanstack/react-router";
import { RouteStub } from "@/ui/RouteStub";

export const Route = createFileRoute("/desktops/")({
  component: Page,
});

function Page() {
  const params = Route.useParams();
  return <RouteStub route="/desktops" purpose="Desktop list: online status, rename, color, pending confirmations badge." params={params} />;
}
