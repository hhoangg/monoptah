import { useQueryClient } from "@tanstack/react-query";
import { useId, useState } from "react";
import { useAction } from "@/features/settings/useAction";
import { cn } from "@/lib/utils";
import { Button } from "@/ui/button";
import { Notice } from "@/ui/Notice";
import { Radio } from "@/ui/Radio";
import { ResponsiveOverlay } from "@/ui/ResponsiveOverlay";
import { blockUser, MAX_REASON_LENGTH, type AdminUser } from "./adminApi";
import { adminErrorMessage, BLOCK_CONFLICT } from "./copy";

export interface BlockDialogProps {
  user: AdminUser;
  /**
   * Everyone below the user in the invite tree that a branch block would also block (still active ones). The
   * acting admin is never in it: the server skips them (security.md 4).
   */
  branch: readonly AdminUser[];
  /** The acting admin is inside the branch (they stay unblocked, so the count excludes them). */
  selfInBranch?: boolean;
  open: boolean;
  onOpenChange: (open: boolean) => void;
  /** Called after the server accepted the block with the number of users it changed. */
  onBlocked: (count: number, cascade: boolean) => void;
}

type Scope = "single" | "branch";

/**
 * Block one user, or the branch (the user and everyone they invited, at any depth). This is the
 * destructive confirm: it states the effects and that unblocking never restores revoked tokens or grants.
 */
export function BlockDialog({ user, branch, selfInBranch = false, open, onOpenChange, onBlocked }: BlockDialogProps) {
  const queryClient = useQueryClient();
  const block = useAction((error) => adminErrorMessage(error, BLOCK_CONFLICT));
  const [scope, setScope] = useState<Scope>("single");
  const [reason, setReason] = useState("");
  const scopeName = useId();
  const reasonId = useId();
  const formId = useId();
  const branchAdmins = branch.filter((member) => member.role === "admin");
  const cascade = scope === "branch";
  const affected = 1 + (cascade ? branch.length : 0);

  async function onSubmit() {
    let changed = 0;
    const ok = await block.run(async () => {
      changed = await blockUser(queryClient, user.id, { cascade, reason: reason.trim() });
    });
    if (ok) {
      onBlocked(changed, cascade);
      onOpenChange(false);
    }
  }

  return (
    <ResponsiveOverlay
      open={open}
      onOpenChange={(next) => (block.busy ? undefined : onOpenChange(next))}
      title={`Block @${user.handle}?`}
      description="Choose who is blocked. You can unblock people later, but that does not undo everything below."
      footer={
        <>
          <Button type="button" variant="ghost" disabled={block.busy} onClick={() => onOpenChange(false)}>
            Cancel
          </Button>
          <Button type="submit" form={formId} variant="destructive" disabled={block.busy} aria-busy={block.busy}>
            {affected === 1 ? `Block @${user.handle}` : `Block ${affected} users`}
          </Button>
        </>
      }
    >
      <form
        id={formId}
        className="grid gap-4"
        onSubmit={(event) => {
          event.preventDefault();
          void onSubmit();
        }}
      >
        <fieldset className="grid gap-2 border-0 p-0">
          <legend className="mb-1 text-label font-medium text-content">Who to block</legend>
          <ScopeOption name={scopeName} value="single" checked={scope === "single"} onChange={setScope} title="This user only" description="People they invited keep their access." />
          <ScopeOption
            name={scopeName}
            value="branch"
            checked={scope === "branch"}
            onChange={setScope}
            title="This user and everyone they invited"
            description={
              branch.length === 0
                ? "They have not invited anyone who is still active, so this blocks only them."
                : `Also blocks ${branch.length} ${branch.length === 1 ? "person" : "people"} they invited, directly or through others.`
            }
          />
        </fieldset>

        {cascade && branchAdmins.length > 0 ? (
          <Notice variant="error" title={`This also blocks ${branchAdmins.length} ${branchAdmins.length === 1 ? "admin" : "admins"}`}>
            {branchAdmins.map((admin) => `@${admin.handle}`).join(", ")}
            {branchAdmins.length === 1 ? " is an admin in this branch and" : " are admins in this branch and"} will lose access to this page.
          </Notice>
        ) : null}
        {cascade && selfInBranch ? <Notice variant="info">You are in this branch. You stay signed in: nobody can block their own account.</Notice> : null}

        <div className="flex flex-col gap-1.5">
          <label htmlFor={reasonId} className="text-label font-medium text-content">
            Reason (optional)
          </label>
          <textarea
            id={reasonId}
            value={reason}
            maxLength={MAX_REASON_LENGTH}
            rows={3}
            onChange={(event) => setReason(event.target.value)}
            className="min-h-20 w-full min-w-0 resize-y rounded-row border border-stroke bg-fill px-4 py-2 text-row text-content placeholder:text-muted-text"
          />
          <p className="flex justify-between gap-3 text-caption text-muted-text">
            <span>Kept with the account. It is not shown to the user and not written to the audit log.</span>
            <span className="tabular shrink-0">{`${reason.length}/${MAX_REASON_LENGTH}`}</span>
          </p>
        </div>

        <div className="rounded-row bg-fill p-3.5 text-small text-content">
          <p className="font-semibold">{affected === 1 ? "What happens to this user" : `What happens to these ${affected} users`}</p>
          <ul className="mt-1.5 list-disc space-y-1 pl-5 text-muted-text">
            <li>They are signed out everywhere and cannot sign in or recover.</li>
            <li>Their desktop tokens and device grants are revoked, their push subscriptions are deleted and live connections are closed.</li>
            <li>Their unused invites are revoked.</li>
            <li>Unblocking restores sign-in only. It never restores revoked tokens or grants: they must pair each desktop again and be re-confirmed.</li>
            {cascade ? <li>Everyone in the branch is blocked together. Unblocking the first user does not unblock the others.</li> : null}
          </ul>
        </div>

        {block.error ? <Notice variant="error">{block.error}</Notice> : null}
        {block.notice ? <Notice variant="info">{block.notice}</Notice> : null}
      </form>
    </ResponsiveOverlay>
  );
}

function ScopeOption({ name, value, checked, onChange, title, description }: { name: string; value: Scope; checked: boolean; onChange: (value: Scope) => void; title: string; description: string }) {
  return (
    <label className={cn("flex min-h-13 cursor-pointer items-start gap-3 rounded-row border border-stroke p-3", checked && "bg-selection")}>
      <Radio name={name} value={value} checked={checked} onChange={() => onChange(value)} className="mt-0.5" />
      <span className="min-w-0">
        <span className="block text-row text-content">{title}</span>
        <span className="block text-meta text-muted-text">{description}</span>
      </span>
    </label>
  );
}
