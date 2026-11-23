// Route of /settings itself. Two jobs:
//  - Old links: the four section hashes of the former single-page settings (#settings-account, #settings-alerts,
//    #settings-look, #settings-app) go to the page that now holds that area.
//  - /settings -> /settings/profile coexists with a list page below 1024px. A wide window has the nav beside every page,
//    so /settings has nothing to show and redirects to the first page. A narrow window has no room for both, so
//    /settings stays the list page: the shell shows the nav (with the quick-find box) full width under the "Settings"
//    header below, and each item pushes into its page, which has a back link here. If a narrow window grows to
//    1024px while on the list, the component navigates to the first page too.
import { createFileRoute, redirect, useNavigate } from "@tanstack/react-router";
import { useEffect } from "react";
import { SettingsExitButton } from "@/features/settings/settingsExit";
import { PageHeader } from "@/ui/PageHeader";
import { useMediaQuery } from "@/ui/useMediaQuery";

const WIDE_QUERY = "(min-width: 64rem)";

const HASH_TARGETS = {
  "settings-account": "/settings/profile",
  "settings-alerts": "/settings/notifications",
  "settings-look": "/settings/appearance",
  "settings-app": "/settings/install",
} as const;

function isWide(): boolean {
  // Without matchMedia (old test environments, no window features) behave as wide: the redirect is the simple path.
  return typeof window.matchMedia !== "function" || window.matchMedia(WIDE_QUERY).matches;
}

export const Route = createFileRoute("/settings/")({
  beforeLoad: ({ location }) => {
    const hash = location.hash.replace(/^#/, "");
    if (Object.hasOwn(HASH_TARGETS, hash)) throw redirect({ to: HASH_TARGETS[hash as keyof typeof HASH_TARGETS], replace: true });
    if (isWide()) throw redirect({ to: "/settings/profile", replace: true });
  },
  component: SettingsList,
});

function SettingsList() {
  const wide = useMediaQuery(WIDE_QUERY);
  const navigate = useNavigate();
  useEffect(() => {
    if (wide) void navigate({ to: "/settings/profile", replace: true });
  }, [wide, navigate]);
  // The nav itself is rendered by the shell; below 1024px this header sits above it.
  return <PageHeader title="Settings" leading={<SettingsExitButton />} className="lg:hidden" />;
}
