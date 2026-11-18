// `/d/:desktopId/monos/:monoId`: the Mono chat (F-69, U-09). Owner: WS-P (workstreams.md 3.1).
import { createFileRoute } from "@tanstack/react-router";
import { MonoScreen } from "@/features/monos/MonoScreen";

export const Route = createFileRoute("/d/$desktopId/monos/$monoId/")({
  component: Page,
});

function Page() {
  const { desktopId, monoId } = Route.useParams();
  return <MonoScreen key={`${desktopId}/${monoId}`} desktopId={desktopId} monoId={monoId} />;
}
