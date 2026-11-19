// `/d/:desktopId/monos`: the Mono home list (F-13). Owner: WS-P (workstreams.md 3.1).
import { createFileRoute } from "@tanstack/react-router";
import { MonoHome } from "@/features/monos/MonoHome";

export const Route = createFileRoute("/d/$desktopId/monos/")({
  component: Page,
});

function Page() {
  const { desktopId } = Route.useParams();
  return <MonoHome desktopId={desktopId} titleAs="h1" />;
}
