// Route of /settings/invites: Invites. A page of the settings shell (routes/settings/route.tsx).
import { createFileRoute } from "@tanstack/react-router";
import { InvitesPage } from "@/features/settings/SettingsPages";

export const Route = createFileRoute("/settings/invites")({
  component: InvitesPage,
});
