import { Link } from "@tanstack/react-router";
import { useQuery } from "@tanstack/react-query";
import { formatDate } from "@/features/settings/format";
import { Badge } from "@/ui/badge";
import { Notice } from "@/ui/Notice";
import { SettingsCard } from "@/ui/SettingsCard";
import { adminSettingsQuery, adminUsersQuery, type AdminSettings, type AdminUser } from "./adminApi";
import { AdminBoundary } from "./AdminBoundary";
import { AdminLayout } from "./AdminLayout";
import { userCountText } from "./AdminHome";
import { buildInviteTree, type TreeNode } from "./tree";

/** Nesting stops indenting here, so a long chain does not squeeze the rows on a phone. */
const MAX_INDENT = 5;

/**
 * /admin/invites (U-04, A-06 tree view). The admin API has no route that lists invite codes, so the
 * tree is built from `invitedBy` of the users list, next to the instance's paused state.
 */
export function AdminInvites() {
  const settings = useQuery(adminSettingsQuery);
  const users = useQuery(adminUsersQuery);
  return (
    <AdminLayout title="Invites" description="Who invited whom, and whether new invites are open." wide>
      <AdminBoundary query={settings} label="admin settings">
        {(data) => <PausedState settings={data} />}
      </AdminBoundary>
      <AdminBoundary query={users} label="the invite tree">
        {({ users: list }) => <InviteTree users={list} />}
      </AdminBoundary>
    </AdminLayout>
  );
}

function PausedState({ settings }: { settings: AdminSettings }) {
  const capReached = settings.maxUsers > 0 && settings.userCount >= settings.maxUsers;
  return (
    <div className="flex flex-col gap-3">
      {settings.invitesPaused ? (
        <Notice variant="info" title="Invites are paused">
          Nobody can create an invite or register, even with an invite already issued. Turn the switch off on the{" "}
          <Link to="/admin" className="underline">
            overview
          </Link>{" "}
          to open invites again.
        </Notice>
      ) : (
        <Notice variant="info" title="Invites are open">
          Members can create invites and new people can register. {`Users: ${userCountText(settings)}.`}
        </Notice>
      )}
      {capReached ? (
        <Notice variant="info" title="The user cap is reached">
          Registration is closed until the cap is raised, even with invites open.
        </Notice>
      ) : null}
    </div>
  );
}

function InviteTree({ users }: { users: readonly AdminUser[] }) {
  const roots = buildInviteTree(users);
  return (
    <SettingsCard
      title="Invite tree"
      footnote="Built from who invited each user. Invite codes themselves are not listed by the server, and a code is shown only once to the person who created it. Blocking a branch blocks everyone below."
    >
      <ul className="m-0 list-none p-0" aria-label="Invite tree">
        {roots.map((node) => (
          <TreeItem key={node.user.id} node={node} depth={0} />
        ))}
      </ul>
    </SettingsCard>
  );
}

function TreeItem({ node, depth }: { node: TreeNode; depth: number }) {
  const { user } = node;
  const invited = node.children.length;
  return (
    <li className="py-0.5">
      <Link
        to="/admin/users"
        search={{ user: user.id }}
        className="flex min-h-11 flex-wrap items-center gap-x-2 gap-y-1 rounded-row px-2 py-1.5 text-row text-content hover:bg-selection"
      >
        <span className="font-medium wrap-anywhere">{`@${user.handle}`}</span>
        {user.role === "admin" ? <Badge variant="outline">Admin</Badge> : null}
        {user.status === "blocked" ? <Badge variant="destructive">Blocked</Badge> : null}
        <span className="text-meta text-muted-text">{`joined ${formatDate(user.createdAt)}${invited > 0 ? `, invited ${invited}` : ""}`}</span>
      </Link>
      {invited > 0 ? (
        <ul className={depth < MAX_INDENT ? "m-0 ml-4 list-none border-l border-stroke pl-2" : "m-0 list-none p-0"} aria-label={`Invited by @${user.handle}`}>
          {node.children.map((child) => (
            <TreeItem key={child.user.id} node={child} depth={depth + 1} />
          ))}
        </ul>
      ) : null}
    </li>
  );
}
