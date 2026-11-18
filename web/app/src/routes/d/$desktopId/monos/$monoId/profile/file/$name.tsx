// `/d/:desktopId/monos/:monoId/profile/file/:name`: a document or the Soul and Memory viewer (F-76, F-79, U-09).
// Owner: WS-P (workstreams.md 3.1).
import { createFileRoute } from "@tanstack/react-router";
import { MonoFileScreen } from "@/features/monos/MonoFileScreen";

export const Route = createFileRoute("/d/$desktopId/monos/$monoId/profile/file/$name")({
  component: Page,
});

function Page() {
  const { desktopId, monoId, name } = Route.useParams();
  return <MonoFileScreen key={`${desktopId}/${monoId}/${name}`} desktopId={desktopId} monoId={monoId} name={name} />;
}
