// `/d/:desktopId/terminal` was the standalone terminal page. The terminal is now a footer chip that opens a tab (or, on a
// phone, a drawer), so an old link or bookmark goes to the workspace home.
import { createFileRoute, redirect } from "@tanstack/react-router";

export const Route = createFileRoute("/d/$desktopId/terminal/")({
  beforeLoad: ({ params }) => {
    throw redirect({ to: "/d/$desktopId", params: { desktopId: params.desktopId }, replace: true });
  },
});
