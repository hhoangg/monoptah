// `/d/:desktopId/monos/:monoId/profile/documents`: the documents of a Mono (F-76, U-09). Owner: WS-P (workstreams.md 3.1).
import { createFileRoute } from "@tanstack/react-router";
import { MonoDocumentsScreen } from "@/features/monos/MonoDocumentsScreen";

export const Route = createFileRoute("/d/$desktopId/monos/$monoId/profile/documents")({
  component: Page,
});

function Page() {
  const { desktopId, monoId } = Route.useParams();
  return <MonoDocumentsScreen key={`${desktopId}/${monoId}`} desktopId={desktopId} monoId={monoId} />;
}
