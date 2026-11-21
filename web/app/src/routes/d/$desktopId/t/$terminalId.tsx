// `/d/:desktopId/t/:terminalId`: a terminal tab (from 640px). The tabbed chat area (features/layout/tabs/TabbedChat.tsx)
// opens the tab for this URL and draws it in its pane, so the route itself renders nothing there. Below 640px there are no
// tabs (the terminal is a drawer), so the URL goes back to the workspace.
import { createFileRoute, Navigate } from "@tanstack/react-router";
import { useMediaQuery } from "@/ui/useMediaQuery";

export const Route = createFileRoute("/d/$desktopId/t/$terminalId")({
  component: Page,
});

function Page() {
  const { desktopId } = Route.useParams();
  const tabbed = useMediaQuery("(min-width: 640px)");
  return tabbed ? null : <Navigate to="/d/$desktopId" params={{ desktopId }} replace />;
}
