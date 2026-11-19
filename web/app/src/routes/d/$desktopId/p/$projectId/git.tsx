// `/d/:desktopId/p/:projectId/git`: branch, ahead/behind, upstream and PR (U-06).
import { createFileRoute } from "@tanstack/react-router";
import "@/features/changes/register";
import { GitScreen } from "@/features/changes/GitScreen";

export const Route = createFileRoute("/d/$desktopId/p/$projectId/git")({
  component: Page,
});

function Page() {
  const { desktopId, projectId } = Route.useParams();
  return <GitScreen key={`${desktopId}/${projectId}`} desktopId={desktopId} projectId={projectId} />;
}
