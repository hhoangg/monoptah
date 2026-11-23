import { useQuery } from "@tanstack/react-query";
import { cn } from "@/lib/utils";
import { SettingsPage } from "@/features/settings/SettingsPage";
import { formatDate } from "@/features/settings/format";
import { Icon } from "@/ui/Icon";
import { SETTINGS_CARD_BODY, SETTINGS_DIVIDER_PLAIN, SETTINGS_HIDE_DIVIDER_SELF_WRAPPED, SETTINGS_ROW_BASE, SettingsCard } from "@/ui/SettingsCard";
import { useMediaQuery } from "@/ui/useMediaQuery";
import { adminUsersQuery, type AdminUser } from "./adminApi";
import { AdminBoundary } from "./AdminBoundary";
import { StatusBadges } from "./StatusBadges";
import { UserDetail } from "./UserDetail";
import { handleOf } from "./tree";

/** From 640px (the start of the tablet range, design-system.md 11) the list and the detail panel sit side by side in the settings column; below it the detail is a page of its own. */
export const TWO_PANE_QUERY = "(min-width: 640px)";

export interface AdminUsersProps {
  selfId: string;
  selectedId: string | undefined;
  onSelect: (id: string | undefined, options?: { resetScroll: boolean }) => void;
}

/**
 * /settings/admin/users (U-04, design-system 11): below 640px the list, then the detail alone (the page's back link
 * returns to the list); from 640px two panes inside the settings column.
 */
export function AdminUsers({ selfId, selectedId, onSelect }: AdminUsersProps) {
  const users = useQuery(adminUsersQuery);
  const twoPane = useMediaQuery(TWO_PANE_QUERY, true);
  // Without `search` the link drops ?user=, so it lands on the list. Only a user in the loaded list has a detail page to leave.
  const selectedLoaded = users.data?.users.some((user) => user.id === selectedId) ?? false;
  const back = !twoPane && selectedLoaded ? { to: "/settings/admin/users" as const, label: "All users" } : undefined;
  // Two panes keep the list in view, so selecting must not jump the window to the top; below 640px the detail replaces the list.
  const select = (id: string) => onSelect(id, { resetScroll: !twoPane });
  return (
    <SettingsPage title="Users" description="Everyone on this instance. Admins never see desktop content." back={back}>
      <AdminBoundary query={users} label="users">
        {({ users: list }) => {
          const selected = list.find((user) => user.id === selectedId);
          if (!twoPane) {
            return selected ? (
              <UserDetail key={selected.id} user={selected} users={list} isSelf={selected.id === selfId} selfId={selfId} />
            ) : (
              <UserList users={list} selectedId={undefined} onSelect={select} narrow />
            );
          }
          return (
            <div className="grid items-start gap-4 grid-cols-[minmax(0,2fr)_minmax(0,3fr)]">
              <UserList users={list} selectedId={selected?.id} onSelect={select} />
              <div className="lg:sticky lg:top-[calc(env(safe-area-inset-top)+2.25rem)] lg:-m-1 lg:max-h-[calc(100dvh-env(safe-area-inset-top)-3.25rem)] self-start lg:overflow-y-auto lg:overscroll-contain lg:p-1">
                {selected ? (
                  <UserDetail key={selected.id} user={selected} users={list} isSelf={selected.id === selfId} selfId={selfId} />
                ) : (
                  <div className={SETTINGS_CARD_BODY}>
                    <p className="px-1.5 py-2 text-small text-muted-text">Select a user to see their details, block them or grant invites.</p>
                  </div>
                )}
              </div>
            </div>
          );
        }}
      </AdminBoundary>
    </SettingsPage>
  );
}

interface UserListProps {
  users: readonly AdminUser[];
  selectedId: string | undefined;
  onSelect: (id: string) => void;
  /** Below 640px the rows lead to a page of their own, so they show a chevron. */
  narrow?: boolean;
}

// The row after a selected one hides its divider too, like the highlighted rows of a settings card.
const HIDE_DIVIDER_SELECTED = "has-[>[aria-current=true]]:before:opacity-0 [:has(>[aria-current=true])+&]:before:opacity-0";

function UserList({ users, selectedId, onSelect, narrow = false }: UserListProps) {
  return (
    <SettingsCard footnote={`${users.length} ${users.length === 1 ? "user" : "users"}, oldest first. Last seen is not reported by the server yet.`}>
      <ul className="m-0 list-none p-0" aria-label="Users">
        {users.map((user) => (
          <li key={user.id} className={cn("relative", SETTINGS_DIVIDER_PLAIN, SETTINGS_HIDE_DIVIDER_SELF_WRAPPED, HIDE_DIVIDER_SELECTED)}>
            <button
              type="button"
              data-row-highlight=""
              onClick={() => onSelect(user.id)}
              aria-label={`@${user.handle}, ${user.displayName}, ${user.status === "blocked" ? "blocked" : "active"}${user.role === "admin" ? ", admin" : ""}`}
              aria-current={user.id === selectedId ? "true" : undefined}
              className={cn(SETTINGS_ROW_BASE, "flex-nowrap hover:bg-selection active:opacity-60", user.id === selectedId && "bg-selection")}
            >
              <span className="flex min-w-0 flex-1 flex-col gap-0.5">
                <span className="flex flex-wrap items-center gap-x-2 gap-y-1">
                  <span className="min-w-0 font-medium wrap-anywhere">{`@${user.handle}`}</span>
                  <span className="ml-auto flex flex-wrap justify-end gap-1.5">
                    <StatusBadges user={user} />
                  </span>
                </span>
                <span className="block text-meta wrap-anywhere text-muted-text">
                  <span className="text-content">{user.displayName}</span>
                  <span aria-hidden="true">{" \u00b7 "}</span>
                  <span>{`Invited by ${inviter(users, user)}, joined ${formatDate(user.createdAt)}, ${user.desktopCount} ${user.desktopCount === 1 ? "desktop" : "desktops"}`}</span>
                </span>
              </span>
              {narrow ? <Icon name="chevronRight" size={18} className="shrink-0 text-faint" /> : null}
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
