// /d/:desktopId/terminal: a shell on the desktop (features.md N-26). The page is a separate chunk (xterm loads only here).
import { createFileRoute } from "@tanstack/react-router";
import { TerminalPage } from "@/features/terminal/TerminalPage";
import { WorkspaceFrame } from "@/features/sessions/WorkspaceFrame";

export const Route = createFileRoute("/d/$desktopId/terminal/")({
  component: Page,
});

function Page() {
  const { desktopId } = Route.useParams();
  return (
    <WorkspaceFrame desktopId={desktopId} title="Terminal" subtitle="A shell on your desktop" testId="terminal-page" fill>
      <TerminalPage />
    </WorkspaceFrame>
  );
}
