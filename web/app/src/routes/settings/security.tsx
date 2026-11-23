// Route of /settings/security: Security (passkeys, recovery codes, signed-in browsers). A page of the settings shell (routes/settings/route.tsx).
import { createFileRoute } from "@tanstack/react-router";
import { SecurityPage } from "@/features/settings/SettingsPages";

export const Route = createFileRoute("/settings/security")({
  component: SecurityPage,
});
