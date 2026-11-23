// Route of /settings/desktops/:id: every setting of one desktop. A page of the settings shell.
import { createFileRoute } from "@tanstack/react-router";
import { DesktopSettingsPage } from "@/features/desktops/DesktopSettingsPage";

export const Route = createFileRoute("/settings/desktops/$id")({
  component: Page,
});

function Page() {
  const { id } = Route.useParams();
  // `key`: the state of one desktop's page must never carry over to another.
  return <DesktopSettingsPage key={id} id={id} />;
}
