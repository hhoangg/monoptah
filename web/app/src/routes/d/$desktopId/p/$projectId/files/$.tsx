// `/d/:desktopId/p/:projectId/files/*`: the file browser and viewer (U-05). The splat is the path ("" is the tree root).
import { createFileRoute } from "@tanstack/react-router";
import "@/features/files/register";
import { cleanPath } from "@/features/files/filePaths";
import { FilesScreen } from "@/features/files/FilesScreen";

export const Route = createFileRoute("/d/$desktopId/p/$projectId/files/$")({
  component: Page,
});

function Page() {
  const { desktopId, projectId, _splat } = Route.useParams();
  return <FilesScreen key={`${desktopId}/${projectId}`} desktopId={desktopId} projectId={projectId} path={cleanPath(_splat)} />;
}
