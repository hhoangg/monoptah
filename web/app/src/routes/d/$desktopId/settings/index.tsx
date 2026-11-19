// Per-desktop settings /d/:desktopId/settings (W-11, F-86). Owner: WS-S (workstreams.md 3.1).
import { createFileRoute } from "@tanstack/react-router";
import { DesktopSettingsScreen } from "@/features/settings/DesktopSettingsScreen";

export const Route = createFileRoute("/d/$desktopId/settings/")({
  component: Page,
});

function Page() {
  const { desktopId } = Route.useParams();
  return <DesktopSettingsScreen desktopId={desktopId} />;
}
