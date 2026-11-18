// `/d/:desktopId/monos/:monoId/profile/habit/:id`: one habit of a Mono (F-77, U-09). Owner: WS-P (workstreams.md 3.1).
import { createFileRoute } from "@tanstack/react-router";
import { MonoHabitScreen } from "@/features/monos/MonoHabitScreen";

export const Route = createFileRoute("/d/$desktopId/monos/$monoId/profile/habit/$id")({
  component: Page,
});

function Page() {
  const { desktopId, monoId, id } = Route.useParams();
  return <MonoHabitScreen key={`${desktopId}/${monoId}/${id}`} desktopId={desktopId} monoId={monoId} habitId={id} />;
}
