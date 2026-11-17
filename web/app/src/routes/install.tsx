// Route stub of /install. Owner: WS-S (workstreams.md 3.1).
import { createFileRoute } from "@tanstack/react-router";
import { RouteStub } from "@/ui/RouteStub";

export const Route = createFileRoute("/install")({
  component: Page,
});

function Page() {
  const params = Route.useParams();
  return <RouteStub route="/install" purpose="Add to Home Screen guide and the push enable button." params={params} />;
}
