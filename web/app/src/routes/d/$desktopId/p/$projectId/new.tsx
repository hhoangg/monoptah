// `/d/:desktopId/p/:projectId/new`: start a session in a project (W-08). Owner: WS-T (workstreams.md 3.1).
import { createFileRoute } from "@tanstack/react-router";
import "@/features/sessions/register";
import { NewSessionScreen } from "@/features/sessions/NewSessionScreen";

export const Route = createFileRoute("/d/$desktopId/p/$projectId/new")({
  component: Page,
});

function Page() {
  const { desktopId, projectId } = Route.useParams();
  return <NewSessionScreen desktopId={desktopId} projectId={projectId} />;
}
