// Route of /desktops/:id (old URL, kept for bookmarks and links): one desktop's page now lives in the settings shell.
// /desktops/:id/verify stays where it is (verify.tsx).
import { createFileRoute, redirect } from "@tanstack/react-router";

export const Route = createFileRoute("/desktops/$id/")({
  beforeLoad: ({ params }) => {
    throw redirect({ to: "/settings/desktops/$id", params: { id: params.id }, replace: true });
  },
});
