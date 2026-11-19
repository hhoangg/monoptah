// `/d/:desktopId/monos/:monoId/profile/edit`: edit a Mono (F-78, U-09). Owner: WS-P (workstreams.md 3.1).
import { createFileRoute } from "@tanstack/react-router";
import { MonoEditScreen } from "@/features/monos/MonoEditScreen";

export const Route = createFileRoute("/d/$desktopId/monos/$monoId/profile/edit")({
  component: Page,
});

function Page() {
  const { desktopId, monoId } = Route.useParams();
  return <MonoEditScreen key={`${desktopId}/${monoId}`} desktopId={desktopId} monoId={monoId} />;
}
