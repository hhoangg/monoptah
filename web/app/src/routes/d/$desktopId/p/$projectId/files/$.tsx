// `/d/:desktopId/p/:projectId/files/*`: the file browser and viewer (U-05). The splat is the path ("" is the tree root).
import { createFileRoute } from "@tanstack/react-router";
import "@/features/files/register";
import { cleanPath } from "@/features/files/filePaths";
import { FilesScreen } from "@/features/files/FilesScreen";

export const Route = createFileRoute("/d/$desktopId/p/$projectId/files/$")({
  // `?line=12`: the line a chat mention (`file.ts:12`) points at, scrolled to and marked in the viewer.
  validateSearch: (search: Record<string, unknown>): { line?: number } => {
    const line = Number(search["line"]);
    return Number.isInteger(line) && line >= 1 && line <= 10_000_000 ? { line } : {};
  },
  component: Page,
});

function Page() {
  const { desktopId, projectId, _splat } = Route.useParams();
  const { line } = Route.useSearch();
  return <FilesScreen key={`${desktopId}/${projectId}`} desktopId={desktopId} projectId={projectId} path={cleanPath(_splat)} {...(line !== undefined ? { line } : {})} />;
}
