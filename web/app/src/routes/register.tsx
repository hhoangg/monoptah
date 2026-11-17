// Route stub of /register. Owner: WS-S (workstreams.md 3.1).
import { createFileRoute } from "@tanstack/react-router";
import { RouteStub } from "@/ui/RouteStub";

export const Route = createFileRoute("/register")({
  component: Page,
});

function Page() {
  const params = Route.useParams();
  return <RouteStub route="/register" purpose="Invite code, handle, display name, passkey creation and the one-time recovery codes." params={params} />;
}
