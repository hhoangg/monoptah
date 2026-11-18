// Route of /desktops. Owner: WS-M (workstreams.md 3.1). Desktop list, pairing help and this account's devices.
import { createFileRoute } from "@tanstack/react-router";
import { DesktopsScreen } from "@/features/desktops/DesktopsScreen";

export const Route = createFileRoute("/desktops/")({
  component: DesktopsScreen,
});
