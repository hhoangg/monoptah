// Route of /settings/notifications: Notifications (this browser, then every desktop). A page of the settings shell (routes/settings/route.tsx).
import { createFileRoute } from "@tanstack/react-router";
import { NotificationsPage } from "@/features/settings/SettingsPages";

export const Route = createFileRoute("/settings/notifications")({
  component: NotificationsPage,
});
