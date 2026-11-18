// `/d/:desktopId/p/:projectId`: the project's sessions list (W-06, F-18, F-21). Owner: WS-T (workstreams.md 3.1).
import { createFileRoute } from "@tanstack/react-router";
import "@/features/sessions/register";
import { ProjectScreen } from "@/features/sessions/screens";

export const Route = createFileRoute("/d/$desktopId/p/$projectId/")({
  component: Page,
});

function Page() {
  const { desktopId, projectId } = Route.useParams();
  return <ProjectScreen desktopId={desktopId} projectId={projectId} />;
}
