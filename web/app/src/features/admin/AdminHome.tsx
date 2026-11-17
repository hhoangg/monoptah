import { useInfiniteQuery, useQuery, useQueryClient } from "@tanstack/react-query";
import { useAction } from "@/features/settings/useAction";
import { formatRelative } from "@/features/settings/format";
import { Button } from "@/ui/button";
import { Notice } from "@/ui/Notice";
import { SettingsCard, SettingsRow } from "@/ui/SettingsCard";
import { Switch } from "@/ui/switch";
import { adminAuditQuery, adminSettingsQuery, adminUsersQuery, setInvitesPaused, type AdminSettings } from "./adminApi";
import { AdminBoundary } from "./AdminBoundary";
import { AdminLayout } from "./AdminLayout";
import { adminErrorMessage, auditDetail, auditTitle } from "./copy";

/** "12 of 150" with a cap, "12 (No cap)" without (`maxUsers` 0 means no cap). */
export function userCountText(settings: AdminSettings): string {
  return settings.maxUsers === 0 ? `${settings.userCount} (No cap)` : `${settings.userCount} of ${settings.maxUsers}`;
}

/** /admin (U-04): the Pause invites switch, the user count against the cap, links, and the audit view. */
export function AdminHome() {
  const settings = useQuery(adminSettingsQuery);
  return (
    <AdminLayout title="Admin" description="Who can join, and what changed.">
      <AdminBoundary query={settings} label="admin settings">
        {(data) => <Overview settings={data} />}
      </AdminBoundary>
    </AdminLayout>
  );
}

function Overview({ settings }: { settings: AdminSettings }) {
  const queryClient = useQueryClient();
  const toggle = useAction((error) => adminErrorMessage(error));
  const capReached = settings.maxUsers > 0 && settings.userCount >= settings.maxUsers;

  return (
    <>
      <SettingsCard
        title="Invites"
        footnote="Pausing takes effect at once: no new invites and no new accounts, even with an invite already issued. Sign-in and existing accounts are not affected."
      >
        <SettingsRow
          icon="userAdd"
          title="Pause invites"
          description={settings.invitesPaused ? "Invites are paused" : "Invites are open"}
          trailing={
            <Switch
              checked={settings.invitesPaused}
              disabled={toggle.busy}
              aria-label="Pause invites"
              onCheckedChange={(next) => void toggle.run(() => setInvitesPaused(queryClient, next))}
            />
          }
        />
        <SettingsRow icon="people" title="Users" description="Accounts on this instance, blocked ones included" trailing={<span className="tabular text-content">{userCountText(settings)}</span>} />
      </SettingsCard>
      {toggle.error ? <Notice variant="error">{toggle.error}</Notice> : null}
      {toggle.notice ? <Notice variant="info">{toggle.notice}</Notice> : null}
      {capReached ? (
        <Notice variant="info" title="The user cap is reached">
          Invites and registration are closed until a user is removed or the cap is raised, even with the switch off.
        </Notice>
      ) : null}

      <SettingsCard title="Manage">
        <SettingsRow icon="people" title="Users" description="Block, unblock and grant invites" to="/admin/users" />
        <SettingsRow icon="link" title="Invites" description="Who invited whom" to="/admin/invites" />
      </SettingsCard>

      <AuditCard />
    </>
  );
}

function AuditCard() {
  const audit = useInfiniteQuery(adminAuditQuery);
  const users = useQuery(adminUsersQuery);
  return (
    <SettingsCard title="Recent activity" footnote="Newest first. The log holds ids and counts only, never block reasons.">
      <AdminBoundary query={{ data: audit.data, error: audit.error, isError: audit.isError, refetch: audit.refetch }} label="recent activity">
        {(data) => {
          const events = data.pages.flatMap((page) => page.events);
          if (events.length === 0) return <p className="px-1.5 py-3 text-small text-muted-text">Nothing has happened yet.</p>;
          return (
            <>
              <ul className="m-0 list-none p-0">
                {events.map((event) => (
                  <SettingsRow
                    as="li"
                    key={event.id}
                    title={auditTitle(event)}
                    description={`${auditDetail(event, users.data?.users)}, ${formatRelative(event.ts)}`}
                  />
                ))}
              </ul>
              {audit.hasNextPage ? (
                <div className="px-1.5 py-2">
                  <Button type="button" variant="outline" size="sm" disabled={audit.isFetchingNextPage} aria-busy={audit.isFetchingNextPage} onClick={() => void audit.fetchNextPage()}>
                    Load more
                  </Button>
                </div>
              ) : null}
            </>
          );
        }}
      </AdminBoundary>
    </SettingsCard>
  );
}
