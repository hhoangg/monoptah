// /d/:desktopId/settings (old URL, kept for links and notifications): the desktop's settings now live in the settings
// shell at /settings/desktops/:desktopId.
import { createFileRoute, redirect } from "@tanstack/react-router";

export const Route = createFileRoute("/d/$desktopId/settings/")({
  beforeLoad: ({ params }) => {
    throw redirect({ to: "/settings/desktops/$id", params: { id: params.desktopId }, replace: true });
  },
});
