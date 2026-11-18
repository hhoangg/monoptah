// `/d/:desktopId`: the catalog home of one desktop (W-06, F-14). Owner: WS-T (workstreams.md 3.1).
import { createFileRoute } from "@tanstack/react-router";
import "@/features/sessions/register";
import { HomeScreen } from "@/features/sessions/screens";

export const Route = createFileRoute("/d/$desktopId/")({
  component: Page,
});

function Page() {
  const { desktopId } = Route.useParams();
  return <HomeScreen desktopId={desktopId} />;
}
