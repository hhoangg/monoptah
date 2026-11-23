// The pages of the settings shell (routes /settings/profile, security, invites, appearance, notifications, install).
// Each is a SettingsPage; the account pages read the signed-in user from the cached `meQuery` (the /settings layout
// route has already loaded it before it renders any page).
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { useNavigate } from "@tanstack/react-router";
import { useState } from "react";
import { loadPauseRecord } from "@/features/push/pauseRecord";
import { meQuery, signOut } from "@/features/accounts/queries";
import { cn } from "@/lib/utils";
import { Button } from "@/ui/button";
import { Icon } from "@/ui/Icon";
import { SETTINGS_ANCHOR } from "@/ui/SettingsCard";
import { Notice } from "@/ui/Notice";
import { Slot } from "@/ui/slots";
import { AccountSection } from "./AccountSection";
import { AppearanceSection } from "./AppearanceSection";
import { InstallGuideContent } from "./InstallGuide";
import { InvitesSection } from "./InvitesSection";
import { NotificationsCard, NotificationsPerDesktop } from "./NotificationsCard";
import { PasskeysSection } from "./PasskeysSection";
import { RecoveryCodesSection } from "./RecoveryCodesSection";
import { SessionsSection } from "./SessionsSection";
import { SettingsPage } from "./SettingsPage";
import { useAction } from "./useAction";

/** Display name, handle and Sign out. */
export function ProfilePage() {
  const me = useQuery(meQuery).data;
  const queryClient = useQueryClient();
  const navigate = useNavigate();
  const logout = useAction();
  if (!me) return null;

  async function onSignOut() {
    const ok = await logout.run(() => signOut(queryClient));
    if (ok) await navigate({ to: "/login" });
  }

  return (
    <SettingsPage title="Profile" description={`Signed in as @${me.user.handle}`}>
      <AccountSection handle={me.user.handle} displayName={me.user.displayName} />
      <div id="sign-out" className={cn("flex flex-col gap-3", SETTINGS_ANCHOR)}>
        {logout.error ? <Notice variant="error">{logout.error}</Notice> : null}
        <Button type="button" variant="outline" className="w-full sm:w-auto sm:self-start" disabled={logout.busy} aria-busy={logout.busy} onClick={() => void onSignOut()}>
          <Icon name="logout" size={18} />
          Sign out
        </Button>
      </div>
    </SettingsPage>
  );
}

/** Passkeys, recovery codes and the signed-in browsers. */
export function SecurityPage() {
  const me = useQuery(meQuery).data;
  if (!me) return null;
  return (
    <SettingsPage title="Security" description="How you sign in and get back in.">
      <PasskeysSection />
      <RecoveryCodesSection handle={me.user.handle} />
      <SessionsSection />
    </SettingsPage>
  );
}

export function InvitesPage() {
  return (
    <SettingsPage title="Invites" description="Invite people to Monoptah.">
      <InvitesSection />
    </SettingsPage>
  );
}

/** Colour scheme, layout, and the cards the `settings-sections` slot adds (Theme, Backgrounds, Monos only). */
export function AppearancePage() {
  return (
    <SettingsPage title="Appearance" browserOnly>
      <AppearanceSection />
      <Slot name="settings-sections" />
    </SettingsPage>
  );
}

/** Permission, test, Pause all and Resume all, then the categories of every desktop. */
export function NotificationsPage() {
  // What Pause all turned off. The per-desktop switches below drop a desktop from it, so the page re-reads it for both.
  const [pausedCount, setPausedCount] = useState(() => Object.keys(loadPauseRecord()).length);
  const reread = () => setPausedCount(Object.keys(loadPauseRecord()).length);
  return (
    <SettingsPage title="Notifications" browserOnly>
      <NotificationsCard pausedCount={pausedCount} onPausedChange={reread} />
      <NotificationsPerDesktop onSaved={reread} />
    </SettingsPage>
  );
}

export function InstallPage() {
  return (
    <SettingsPage title="Install app" description="Open Monoptah like any other app, full screen and one tap away." browserOnly>
      <InstallGuideContent id="install" />
    </SettingsPage>
  );
}
