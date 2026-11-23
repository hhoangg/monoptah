// Route of /settings/profile: Profile (display name, handle, sign out). A page of the settings shell (routes/settings/route.tsx).
import { createFileRoute } from "@tanstack/react-router";
import { ProfilePage } from "@/features/settings/SettingsPages";

export const Route = createFileRoute("/settings/profile")({
  component: ProfilePage,
});
