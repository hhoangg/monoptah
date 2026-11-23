// Route of /desktops (old URL, kept for bookmarks and links): the desktop list now lives in the settings shell.
import { createFileRoute, redirect } from "@tanstack/react-router";

export const Route = createFileRoute("/desktops/")({
  beforeLoad: () => {
    throw redirect({ to: "/settings/desktops", replace: true });
  },
});
