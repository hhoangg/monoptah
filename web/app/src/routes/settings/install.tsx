// Route of /settings/install: Install app (this browser only). A page of the settings shell (routes/settings/route.tsx).
import { createFileRoute } from "@tanstack/react-router";
import { InstallPage } from "@/features/settings/SettingsPages";

export const Route = createFileRoute("/settings/install")({
  component: InstallPage,
});
