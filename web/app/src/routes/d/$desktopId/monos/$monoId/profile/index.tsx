// `/d/:desktopId/monos/:monoId/profile`: the Mono profile (F-76, U-09). Owner: WS-P (workstreams.md 3.1).
import { createFileRoute } from "@tanstack/react-router";
import { MonoProfileScreen } from "@/features/monos/MonoProfileScreen";

export const Route = createFileRoute("/d/$desktopId/monos/$monoId/profile/")({
  component: Page,
});

function Page() {
  const { desktopId, monoId } = Route.useParams();
  return <MonoProfileScreen key={`${desktopId}/${monoId}`} desktopId={desktopId} monoId={monoId} />;
}
