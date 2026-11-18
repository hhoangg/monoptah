// `/d/:desktopId/p/:projectId/changes/*`: the changes list (empty splat) or the diff of a file (U-06).
import { createFileRoute } from "@tanstack/react-router";
import "@/features/changes/register";
import { ChangesScreen } from "@/features/changes/ChangesScreen";
import { cleanPath } from "@/features/files/filePaths";

export const Route = createFileRoute("/d/$desktopId/p/$projectId/changes/$")({
  component: Page,
});

function Page() {
  const { desktopId, projectId, _splat } = Route.useParams();
  return <ChangesScreen key={`${desktopId}/${projectId}`} desktopId={desktopId} projectId={projectId} path={cleanPath(_splat)} />;
}
