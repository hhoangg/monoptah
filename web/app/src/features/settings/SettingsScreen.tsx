import { useQueryClient } from "@tanstack/react-query";
import { useNavigate } from "@tanstack/react-router";
import { signOut } from "@/features/accounts/queries";
import { Button } from "@/ui/button";
import { Icon } from "@/ui/Icon";
import { Notice } from "@/ui/Notice";
import { PageHeader } from "@/ui/PageHeader";
import { SettingsCard, SettingsRow } from "@/ui/SettingsCard";
import { Slot } from "@/ui/slots";
import { AccountSection } from "./AccountSection";
import { AppearanceSection } from "./AppearanceSection";
import { NotificationsCard } from "./NotificationsCard";
import { InvitesSection } from "./InvitesSection";
import { PasskeysSection } from "./PasskeysSection";
import { RecoveryCodesSection } from "./RecoveryCodesSection";
import { SessionsSection } from "./SessionsSection";
import { SettingsLayout } from "./SettingsLayout";
import { useAction } from "./useAction";

export interface SettingsScreenProps {
  handle: string;
  displayName: string;
  /** Only admins get the link to /admin. */
  role?: "user" | "admin";
}

/**
 * Account settings (W-11). One centered column of SettingsCard groups on every width (see the layout
 * note in SettingsLayout). `settings-sections` is the slot WS-P fills with its notification group.
 */
export function SettingsScreen({ handle, displayName, role }: SettingsScreenProps) {
  const queryClient = useQueryClient();
  const navigate = useNavigate();
  const logout = useAction();

  async function onSignOut() {
    const ok = await logout.run(() => signOut(queryClient));
    if (ok) await navigate({ to: "/login" });
  }

  return (
    <SettingsLayout>
      <PageHeader title="Settings" description={`Signed in as @${handle}`} />
      <AccountSection handle={handle} displayName={displayName} />
      <PasskeysSection />
      <RecoveryCodesSection handle={handle} />
      <SessionsSection />
      <NotificationsCard />
      <InvitesSection />
      <AppearanceSection />
      <Slot name="settings-sections" />
      {role === "admin" ? (
        <SettingsCard title="Administration">
          <SettingsRow icon="admin" title="Admin" description="Users, invites and the audit log" to="/admin" />
        </SettingsCard>
      ) : null}
      <SettingsCard title="App">
        <SettingsRow icon="download" title="Install Monoptah" description="Add it to your home screen or desktop" to="/install" />
      </SettingsCard>
      <div className="flex flex-col gap-3">
        {logout.error ? <Notice variant="error">{logout.error}</Notice> : null}
        <Button type="button" variant="outline" className="w-full sm:w-auto sm:self-start" disabled={logout.busy} aria-busy={logout.busy} onClick={() => void onSignOut()}>
          <Icon name="logout" size={18} />
          Sign out
        </Button>
      </div>
    </SettingsLayout>
  );
}
