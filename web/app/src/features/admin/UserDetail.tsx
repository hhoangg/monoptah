import { useQueryClient } from "@tanstack/react-query";
import { useState } from "react";
import { useAction } from "@/features/settings/useAction";
import { formatDate } from "@/features/settings/format";
import { Button } from "@/ui/button";
import { ConfirmDialog } from "@/ui/ConfirmDialog";
import { Field } from "@/ui/Field";
import { Icon } from "@/ui/Icon";
import { Notice } from "@/ui/Notice";
import { SettingsCard, SettingsRow } from "@/ui/SettingsCard";
import { grantInvites, MAX_GRANT, unblockUser, type AdminUser } from "./adminApi";
import { BlockDialog } from "./BlockDialog";
import { adminErrorMessage, GRANT_CONFLICT, UNBLOCK_CONFLICT } from "./copy";
import { descendantsOf, handleOf } from "./tree";

export interface UserDetailProps {
  user: AdminUser;
  users: readonly AdminUser[];
  isSelf: boolean;
  /** Mobile: leaves the detail and returns to the list. */
  onBack?: () => void;
}

/** Whether `count` is a whole number the grant route accepts (1 to 100). */
export function isValidGrant(count: number): boolean {
  return Number.isInteger(count) && count >= 1 && count <= MAX_GRANT;
}

export function statusText(user: AdminUser, users: readonly AdminUser[]): string {
  if (user.status === "active") return "Active";
  if (user.blockedVia === null) return "Blocked";
  if (user.blockedVia === user.id) return "Blocked, together with everyone they invited";
  const root = handleOf(users, user.blockedVia);
  return root ? `Blocked, with the branch of ${root}` : "Blocked, with a branch";
}

/** The detail panel of one user: facts, Block, Unblock and Grant invites. */
export function UserDetail({ user, users, isSelf, onBack }: UserDetailProps) {
  const queryClient = useQueryClient();
  const [blocking, setBlocking] = useState(false);
  const [unblocking, setUnblocking] = useState(false);
  const [result, setResult] = useState<string | null>(null);
  const unblock = useAction((error) => adminErrorMessage(error, UNBLOCK_CONFLICT));
  const branch = descendantsOf(users, user.id).filter((member) => member.status === "active" && member.id !== user.id);
  const invitedCount = users.filter((member) => member.invitedBy === user.id).length;
  const reachable = descendantsOf(users, user.id).length;

  async function onUnblock() {
    const ok = await unblock.run(() => unblockUser(queryClient, user.id));
    if (ok) {
      setUnblocking(false);
      setResult(`Unblocked @${user.handle}. They can sign in again.`);
    }
  }

  return (
    <section aria-labelledby="user-detail-title" className="flex flex-col gap-4">
      {onBack ? (
        <Button type="button" variant="ghost" size="sm" className="self-start" onClick={onBack}>
          <Icon name="chevronRight" size={18} className="rotate-180" />
          All users
        </Button>
      ) : null}
      <header className="min-w-0">
        <h2 id="user-detail-title" className="text-title font-semibold wrap-anywhere text-content">
          {`@${user.handle}`}
        </h2>
        <p className="text-body wrap-anywhere text-muted-text">{user.displayName}</p>
      </header>

      <SettingsCard>
        <SettingsRow title="Status" description={statusText(user, users)} />
        <SettingsRow title="Role" description={user.role === "admin" ? "Administrator" : "Member"} />
        <SettingsRow title="Invited by" description={user.invitedBy === null ? "Bootstrap invite" : (handleOf(users, user.invitedBy) ?? "A user that no longer exists")} />
        <SettingsRow title="Joined" description={formatDate(user.createdAt)} />
        <SettingsRow title="Desktops" description={String(user.desktopCount)} />
        <SettingsRow title="Invites granted" description={String(user.invitesGranted)} />
        <SettingsRow title="People invited" description={reachable === invitedCount ? String(invitedCount) : `${invitedCount} directly, ${reachable} in total`} />
      </SettingsCard>

      {result ? <Notice variant="info">{result}</Notice> : null}

      <div className="flex flex-col gap-3">
        {isSelf ? <Notice variant="info">This is your account. You cannot block yourself.</Notice> : null}
        {user.status === "blocked" ? (
          <>
            <p className="text-small text-muted-text">
              Unblocking restores sign-in for this user only. People they invited stay blocked until you unblock them one by one, and revoked desktop tokens and grants are never restored.
            </p>
            <Button type="button" variant="outline" className="self-start" onClick={() => { unblock.reset(); setUnblocking(true); }}>
              Unblock @{user.handle}
            </Button>
          </>
        ) : !isSelf ? (
          <Button type="button" variant="outline" className="self-start text-deletions" onClick={() => setBlocking(true)}>
            <Icon name="shield" size={18} />
            Block @{user.handle}
          </Button>
        ) : null}
      </div>

      <GrantInvites key={user.id} user={user} onGranted={(count) => setResult(`Granted ${count} ${count === 1 ? "invite" : "invites"} to @${user.handle}.`)} />

      <BlockDialog
        user={user}
        branch={branch}
        open={blocking}
        onOpenChange={setBlocking}
        onBlocked={(count, cascade) => setResult(cascade ? `Blocked ${count} ${count === 1 ? "user" : "users"} in the branch of @${user.handle}.` : `Blocked @${user.handle}.`)}
      />
      <ConfirmDialog
        open={unblocking}
        onOpenChange={setUnblocking}
        title={`Unblock @${user.handle}?`}
        description="They can sign in again. Their desktop pairings, device grants, push subscriptions and revoked invites are not restored: they must pair each desktop again and be re-confirmed. People they invited stay blocked until you unblock them one by one."
        confirmLabel="Unblock"
        busy={unblock.busy}
        error={unblock.error}
        notice={unblock.notice}
        onConfirm={() => void onUnblock()}
      />
    </section>
  );
}

function GrantInvites({ user, onGranted }: { user: AdminUser; onGranted: (count: number) => void }) {
  const queryClient = useQueryClient();
  const grant = useAction((error) => adminErrorMessage(error, GRANT_CONFLICT));
  const [value, setValue] = useState("1");
  const [invalid, setInvalid] = useState<string | null>(null);

  async function onSubmit() {
    const count = Number(value);
    if (value.trim() === "" || !isValidGrant(count)) {
      setInvalid(`Enter a whole number from 1 to ${MAX_GRANT}.`);
      return;
    }
    setInvalid(null);
    const ok = await grant.run(() => grantInvites(queryClient, user.id, count));
    if (ok) onGranted(count);
  }

  return (
    <form
      noValidate
      className="flex flex-col gap-3 rounded-card bg-surface p-4"
      onSubmit={(event) => {
        event.preventDefault();
        void onSubmit();
      }}
    >
      <Field
        label="Grant invites"
        type="number"
        inputMode="numeric"
        min={1}
        max={MAX_GRANT}
        step={1}
        value={value}
        onChange={(event) => setValue(event.target.value)}
        hint={`Adds to the invites @${user.handle} can create. From 1 to ${MAX_GRANT} at a time.`}
        error={invalid}
      />
      {grant.error ? <Notice variant="error">{grant.error}</Notice> : null}
      {grant.notice ? <Notice variant="info">{grant.notice}</Notice> : null}
      <Button type="submit" variant="outline" className="self-start" disabled={grant.busy} aria-busy={grant.busy}>
        <Icon name="userAdd" size={18} />
        Grant invites
      </Button>
    </form>
  );
}
