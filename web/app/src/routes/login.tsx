// Route stub of /login. Owner: WS-S (workstreams.md 3.1).
import { createFileRoute } from "@tanstack/react-router";
import { RouteStub } from "@/ui/RouteStub";

export const Route = createFileRoute("/login")({
  component: Page,
});

function Page() {
  const params = Route.useParams();
  return <RouteStub route="/login" purpose="Passkey sign-in with a link to /recover." params={params} />;
}
