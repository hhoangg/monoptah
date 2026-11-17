// Route stub of /pair/:code. Owner: WS-S (workstreams.md 3.1).
import { createFileRoute } from "@tanstack/react-router";
import { RouteStub } from "@/ui/RouteStub";

export const Route = createFileRoute("/pair/$code")({
  component: Page,
});

function Page() {
  const params = Route.useParams();
  return <RouteStub route="/pair/:code" purpose="Approve a desktop: machine name, fingerprint and confirmation code." params={params} />;
}
