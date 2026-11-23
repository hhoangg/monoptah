// Route of /admin/invites (old URL, kept for bookmarks and links): the invite tree now lives in the settings shell.
import { createFileRoute, redirect } from "@tanstack/react-router";

export const Route = createFileRoute("/admin/invites")({
  beforeLoad: () => {
    throw redirect({ to: "/settings/admin/invites", replace: true });
  },
});
