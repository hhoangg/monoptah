// `/d/:desktopId/s/:sessionId`: the transcript of one session (W-07), and the composer (W-09, Task 5). Owner: WS-T (workstreams.md 3.1).
import { createFileRoute } from "@tanstack/react-router";
import "@/features/sessions/register";
import { SessionScreen } from "@/features/sessions/SessionScreen";

export const Route = createFileRoute("/d/$desktopId/s/$sessionId")({
  component: Page,
});

function Page() {
  const { desktopId, sessionId } = Route.useParams();
  // Keyed by the session: a new session starts with its own pages and scroll state.
  return <SessionScreen key={sessionId} desktopId={desktopId} sessionId={sessionId} />;
}
