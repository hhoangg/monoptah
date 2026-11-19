import type { ReactNode } from "react";
import { useQueryClient } from "@tanstack/react-query";
import { useNavigate } from "@tanstack/react-router";
import { signOut } from "@/features/accounts/queries";
import { Button } from "@/ui/button";
import { Icon } from "@/ui/Icon";
import { Notice } from "@/ui/Notice";
import { PageHeader } from "@/ui/PageHeader";
import { SettingsCard, SettingsCardsUnderHeading, SettingsRow } from "@/ui/SettingsCard";
import { Slot } from "@/ui/slots";
import { AccountSection } from "./AccountSection";
import { AppearanceSection } from "./AppearanceSection";
import { NotificationsCard } from "./NotificationsCard";
import { InvitesSection } from "./InvitesSection";
import { PasskeysSection } from "./PasskeysSection";
import { RecoveryCodesSection } from "./RecoveryCodesSection";
import { SessionsSection } from "./SessionsSection";
import { SectionPills, SettingsLayout, type SectionLink } from "./SettingsLayout";
import { useAction } from "./useAction";

export interface SettingsScreenProps {
  handle: string;
  displayName: string;
  /** Only admins get the link to /admin. */
  role?: "user" | "admin";
}

/**
 * Account settings (W-11). SettingsCard groups under four headed areas with a section index (see SettingsLayout). `settings-sections` is the slot WS-P fills with its notification group.
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
    <SettingsLayout sections={AREAS}>
      <PageHeader title="Settings" description={`Signed in as @${handle}`} />
      <SectionPills sections={AREAS} />

      <Area id="settings-account" title="Account and security">
        <AccountSection handle={handle} displayName={displayName} />
        <div className="flex flex-col gap-3">
          {logout.error ? <Notice variant="error">{logout.error}</Notice> : null}
          <Button type="button" variant="outline" className="w-full sm:w-auto sm:self-start" disabled={logout.busy} aria-busy={logout.busy} onClick={() => void onSignOut()}>
            <Icon name="logout" size={18} />
            Sign out
          </Button>
        </div>
        <PasskeysSection />
        <RecoveryCodesSection handle={handle} />
        <SessionsSection />
        <InvitesSection />
      </Area>

      <Area id="settings-alerts" title="Alerts">
        <NotificationsCard />
      </Area>

      <Area id="settings-look" title="Look and feel">
        <AppearanceSection />
        <Slot name="settings-sections" />
      </Area>

      <Area id="settings-app" title="App and admin">
        {role === "admin" ? (
          <SettingsCard title="Administration">
            <SettingsRow icon="admin" title="Admin" description="Users, invites and the audit log" to="/admin" />
          </SettingsCard>
        ) : null}
        <SettingsCard title="App">
          <SettingsRow icon="download" title="Install Monoptah" description="Add it to your home screen or desktop" to="/install" />
        </SettingsCard>
      </Area>
    </SettingsLayout>
  );
}

const AREAS: readonly SectionLink[] = [
  { id: "settings-account", label: "Account and security" },
  { id: "settings-alerts", label: "Alerts" },
  { id: "settings-look", label: "Look and feel" },
  { id: "settings-app", label: "App and admin" },
];

/** A headed area of the page: the target of one index link, holding several cards. */
function Area({ id, title, children }: { id: string; title: string; children: ReactNode }) {
  return (
    <div id={id} className="flex scroll-mt-[calc(env(safe-area-inset-top)+3.5rem)] flex-col gap-6">
      <h2 className="border-b border-stroke px-1 pb-2 text-h font-semibold text-content">{title}</h2>
      <SettingsCardsUnderHeading>{children}</SettingsCardsUnderHeading>
    </div>
  );
}
