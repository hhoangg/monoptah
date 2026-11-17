// Route stub of /recover. Owner: WS-S (workstreams.md 3.1).
import { createFileRoute } from "@tanstack/react-router";
import { RouteStub } from "@/ui/RouteStub";

export const Route = createFileRoute("/recover")({
  component: Page,
});

function Page() {
  const params = Route.useParams();
  return <RouteStub route="/recover" purpose="Handle and recovery code, then a forced new passkey." params={params} />;
}
