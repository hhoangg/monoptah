// Route stub of /desktops/:id/verify. Owner: WS-M (workstreams.md 3.1).
import { createFileRoute } from "@tanstack/react-router";
import { RouteStub } from "@/ui/RouteStub";

export const Route = createFileRoute("/desktops/$id/verify")({
  component: Page,
});

function Page() {
  const params = Route.useParams();
  return <RouteStub route="/desktops/:id/verify" purpose="Re-pin the desktop key after a key change by comparing the #fp fragment." params={params} />;
}
