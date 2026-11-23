// Route of /admin (old URL, kept for bookmarks and links): the admin overview now lives in the settings shell.
import { createFileRoute, redirect } from "@tanstack/react-router";

export const Route = createFileRoute("/admin/")({
  beforeLoad: () => {
    throw redirect({ to: "/settings/admin", replace: true });
  },
});
