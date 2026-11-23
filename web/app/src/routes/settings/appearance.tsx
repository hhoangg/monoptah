// Route of /settings/appearance: Appearance (this browser only). A page of the settings shell (routes/settings/route.tsx).
import { createFileRoute } from "@tanstack/react-router";
import { AppearancePage } from "@/features/settings/SettingsPages";

export const Route = createFileRoute("/settings/appearance")({
  component: AppearancePage,
});
