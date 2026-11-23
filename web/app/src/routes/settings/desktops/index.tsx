// Route of /settings/desktops: desktop list, pairing help and this account's browsers. A page of the settings shell.
import { createFileRoute } from "@tanstack/react-router";
import { DesktopsScreen } from "@/features/desktops/DesktopsScreen";

export const Route = createFileRoute("/settings/desktops/")({
  component: DesktopsScreen,
});
