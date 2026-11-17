import { useQuery } from "@tanstack/react-query";
import { cn } from "@/lib/utils";
import { formatDate } from "@/features/settings/format";
import { Badge } from "@/ui/badge";
import { SettingsCard } from "@/ui/SettingsCard";
import { useMediaQuery } from "@/ui/useMediaQuery";
import { adminUsersQuery, type AdminUser } from "./adminApi";
import { AdminBoundary } from "./AdminBoundary";
import { AdminLayout } from "./AdminLayout";
import { UserDetail } from "./UserDetail";
import { handleOf } from "./tree";

/** From 768px the list and the detail panel sit side by side; below it the screens are full-screen routes. */
export const TWO_PANE_QUERY = "(min-width: 768px)";

export interface AdminUsersProps {
  selfId: string;
  selectedId: string | undefined;
  onSelect: (id: string | undefined) => void;
}

/**
 * /admin/users (U-04, design-system 11 "Desktop manager, admin"): mobile shows the list, then a
 * full-screen detail; tablet is two panes; desktop (1024+) a table-like list with a detail panel.
 */
export function AdminUsers({ selfId, selectedId, onSelect }: AdminUsersProps) {
  const users = useQuery(adminUsersQuery);
  const twoPane = useMediaQuery(TWO_PANE_QUERY, true);
  return (
    <AdminLayout title="Users" description="Everyone on this instance. Admins never see desktop content." wide>
      <AdminBoundary query={users} label="users">
        {({ users: list }) => {
          const selected = list.find((user) => user.id === selectedId);
          if (!twoPane) {
            return selected ? (
              <UserDetail key={selected.id} user={selected} users={list} isSelf={selected.id === selfId} onBack={() => onSelect(undefined)} />
            ) : (
              <UserList users={list} selectedId={undefined} onSelect={onSelect} />
            );
          }
          return (
            <div className="grid items-start gap-5 md:grid-cols-[minmax(0,2fr)_minmax(0,3fr)] lg:grid-cols-[minmax(0,3fr)_minmax(0,2fr)]">
              <UserList users={list} selectedId={selected?.id} onSelect={onSelect} />
              {selected ? (
                <UserDetail key={selected.id} user={selected} users={list} isSelf={selected.id === selfId} />
              ) : (
                <p className="rounded-card bg-surface p-5 text-small text-muted-text">Select a user to see their details, block them or grant invites.</p>
              )}
            </div>
          );
        }}
      </AdminBoundary>
    </AdminLayout>
  );
}

const COLUMNS = "lg:grid lg:grid-cols-[minmax(0,2fr)_minmax(0,1.3fr)_6.5rem_4.5rem_8rem] lg:items-center lg:gap-3";

function UserList({ users, selectedId, onSelect }: { users: readonly AdminUser[]; selectedId: string | undefined; onSelect: (id: string) => void }) {
  return (
    <SettingsCard footnote={`${users.length} ${users.length === 1 ? "user" : "users"}, oldest first. Last seen is not reported by the server yet.`}>
      <div aria-hidden="true" className={cn("hidden px-1.5 py-2 text-meta font-medium text-muted-text", COLUMNS)}>
        <span>User</span>
        <span>Invited by</span>
        <span>Created</span>
        <span>Desktops</span>
        <span>Status</span>
      </div>
      <ul className="m-0 list-none p-0" aria-label="Users">
        {users.map((user) => (
          <li key={user.id} className="relative before:absolute before:top-0 before:right-0 before:left-1.5 before:h-px before:bg-stroke first:before:hidden">
            <button
              type="button"
              onClick={() => onSelect(user.id)}
              aria-label={`@${user.handle}, ${user.displayName}, ${user.status === "blocked" ? "blocked" : "active"}${user.role === "admin" ? ", admin" : ""}`}
              aria-current={user.id === selectedId ? "true" : undefined}
              className={cn("min-h-13 w-full rounded-row px-1.5 py-2 text-left text-row text-content hover:bg-selection", user.id === selectedId && "bg-selection", COLUMNS)}
            >
              <span className="flex min-w-0 items-center gap-2">
                <span className="min-w-0 flex-1">
                  <span className="block font-medium wrap-anywhere">{`@${user.handle}`}</span>
                  <span className="block text-meta wrap-anywhere text-muted-text">{user.displayName}</span>
                </span>
                <span className="flex shrink-0 gap-1.5 lg:hidden">
                  <StatusBadges user={user} />
                </span>
              </span>
              <span className="mt-0.5 block text-meta text-muted-text lg:hidden">
                {`Invited by ${inviter(users, user)}, joined ${formatDate(user.createdAt)}, ${user.desktopCount} ${user.desktopCount === 1 ? "desktop" : "desktops"}`}
              </span>
              <span className="hidden text-small wrap-anywhere text-muted-text lg:block">{inviter(users, user)}</span>
              <span className="hidden text-small text-muted-text lg:block">{formatDate(user.createdAt)}</span>
              <span className="tabular hidden text-small text-muted-text lg:block">{user.desktopCount}</span>
              <span className="hidden flex-wrap gap-1.5 lg:flex">
                <StatusBadges user={user} />
              </span>
            </button>
          </li>
        ))}
      </ul>
    </SettingsCard>
  );
}

function inviter(users: readonly AdminUser[], user: AdminUser): string {
  return user.invitedBy === null ? "Bootstrap" : (handleOf(users, user.invitedBy) ?? "Unknown");
}

export function StatusBadges({ user }: { user: AdminUser }) {
  return (
    <>
      {user.role === "admin" ? <Badge variant="outline">Admin</Badge> : null}
      {user.status === "blocked" ? <Badge variant="destructive">Blocked</Badge> : <Badge variant="outline">Active</Badge>}
    </>
  );
}
