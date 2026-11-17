// Layout of /d/:desktopId: mounts the LinkProvider placeholder and the Wave 0 slots. Owner: WS-L.
import { createFileRoute, Outlet } from "@tanstack/react-router";
import { LinkProvider } from "@/link/LinkProvider";
import { Slot } from "@/ui/slots";

export const Route = createFileRoute("/d/$desktopId")({
  component: DesktopLayout,
});

function DesktopLayout() {
  const { desktopId } = Route.useParams();
  return (
    <LinkProvider desktopId={desktopId}>
      <Slot name="desktop-switcher" />
      <Slot name="link-state-screens" />
      <Slot name="first-run-picker" />
      <Slot name="app-shell">
        <Outlet />
      </Slot>
    </LinkProvider>
  );
}
