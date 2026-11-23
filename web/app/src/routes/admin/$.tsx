// Any other old /admin/* URL (kept for bookmarks and links): the admin area now lives in the settings shell, so an
// unknown sub path lands on its overview.
import { createFileRoute, redirect } from "@tanstack/react-router";

export const Route = createFileRoute("/admin/$")({
  beforeLoad: () => {
    throw redirect({ to: "/settings/admin", replace: true });
  },
});
