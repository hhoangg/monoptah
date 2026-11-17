// Route stub of /settings. Owner: WS-S (workstreams.md 3.1).
import { createFileRoute } from "@tanstack/react-router";
import { RouteStub } from "@/ui/RouteStub";

export const Route = createFileRoute("/settings/")({
  component: Page,
});

function Page() {
  const params = Route.useParams();
  return <RouteStub route="/settings" purpose="Account: passkeys, recovery codes, active sessions, notifications, theme, install." params={params} />;
}
