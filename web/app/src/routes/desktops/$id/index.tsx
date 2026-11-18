// Route of /desktops/:id. Owner: WS-M (workstreams.md 3.1). Manage one desktop: rename, color, devices and grants, revoke.
import { createFileRoute } from "@tanstack/react-router";
import { DesktopDetailScreen } from "@/features/desktops/DesktopDetailScreen";

export const Route = createFileRoute("/desktops/$id/")({
  component: Page,
});

function Page() {
  const { id } = Route.useParams();
  return <DesktopDetailScreen id={id} />;
}
