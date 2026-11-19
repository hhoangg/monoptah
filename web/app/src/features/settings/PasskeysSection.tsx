import { useQuery, useQueryClient } from "@tanstack/react-query";
import { useState } from "react";
import type { PasskeysResponse } from "@monoptah/protocol";
import { addPasskey, deletePasskey, passkeysQuery, renamePasskey } from "@/features/accounts/queries";
import { passkeysSupported } from "@/features/accounts/webauthn";
import { Button } from "@/ui/button";
import { ConfirmDialog } from "@/ui/ConfirmDialog";
import { Dialog, DialogContent, DialogDescription, DialogFooter, DialogHeader, DialogTitle } from "@/ui/dialog";
import { Field } from "@/ui/Field";
import { Icon } from "@/ui/Icon";
import { Notice } from "@/ui/Notice";
import { SettingsCard, SettingsRow } from "@/ui/SettingsCard";
import { formatDate, formatRelative } from "./format";
import { QueryBoundary } from "./QueryBoundary";
import { useAction } from "./useAction";

type Passkey = ReturnType<typeof PasskeysResponse.parse>["passkeys"][number];

const LAST_PASSKEY_NOTE = "You need at least one passkey, so the only one cannot be removed. Add another passkey first.";

function details(passkey: Passkey): string {
  const parts = [`Added ${formatDate(passkey.createdAt)}`, passkey.lastUsedAt ? `last used ${formatRelative(passkey.lastUsedAt)}` : "never used"];
  parts.push(passkey.backedUp ? "backed up" : "this device only");
  return parts.join(", ");
}

/** Security > Passkeys (N-02): list, add, rename and remove. All three are fresh-auth actions. */
export function PasskeysSection() {
  const query = useQuery(passkeysQuery);
  return (
    <SettingsCard title="Passkeys">
      <QueryBoundary query={query} label="passkeys">
        {(data) => <PasskeyList passkeys={[...data.passkeys].sort((a, b) => a.createdAt - b.createdAt)} />}
      </QueryBoundary>
    </SettingsCard>
  );
}

function PasskeyList({ passkeys }: { passkeys: Passkey[] }) {
  const queryClient = useQueryClient();
  const add = useAction();
  const rename = useAction();
  const remove = useAction();
  const [renaming, setRenaming] = useState<Passkey | null>(null);
  const [name, setName] = useState("");
  const [removing, setRemoving] = useState<Passkey | null>(null);
  const [addedNotice, setAddedNotice] = useState<string | null>(null);
  const last = passkeys.length <= 1;
  const supported = passkeysSupported();

  async function onAdd() {
    setAddedNotice(null);
    let cancelled = false;
    const ok = await add.run(async () => {
      const result = await addPasskey(queryClient);
      cancelled = result.status === "cancelled";
    });
    if (ok && cancelled) setAddedNotice("The passkey prompt was closed. No passkey was added.");
  }

  async function onRename() {
    if (!renaming) return;
    if (await rename.run(() => renamePasskey(queryClient, renaming.id, name.trim()))) setRenaming(null);
  }

  async function onRemove() {
    if (!removing) return;
    if (await remove.run(() => deletePasskey(queryClient, removing.id))) setRemoving(null);
  }

  return (
    <>
      <ul className="m-0 list-none p-0">
        {passkeys.map((passkey) => (
          <SettingsRow
              as="li"
              key={passkey.id}
              icon="fingerprint"
              title={passkey.name}
              description={details(passkey)}
              trailing={
                <span className="flex items-center gap-1">
                  <Button
                    type="button"
                    variant="ghost"
                    size="circle"
                    aria-label={`Rename passkey ${passkey.name}`}
                    onClick={() => {
                      setName(passkey.name);
                      rename.reset();
                      setRenaming(passkey);
                    }}
                  >
                    <Icon name="edit" size={18} />
                  </Button>
                  <Button
                    type="button"
                    variant="ghost"
                    size="circle"
                    aria-label={`Remove passkey ${passkey.name}`}
                    disabled={last}
                    aria-describedby={last ? "last-passkey-note" : undefined}
                    onClick={() => {
                      remove.reset();
                      setRemoving(passkey);
                    }}
                  >
                    <Icon name="trash" size={18} />
                  </Button>
                </span>
              }
            />
        ))}
      </ul>
      {last ? (
        <p id="last-passkey-note" className="px-1.5 py-2 text-meta text-muted-text">
          {LAST_PASSKEY_NOTE}
        </p>
      ) : null}
      <div className="flex flex-col gap-3 px-1.5 py-2">
        {!supported ? <Notice variant="error">This browser cannot use passkeys.</Notice> : null}
        {add.error ? <Notice variant="error">{add.error}</Notice> : null}
        {add.notice || addedNotice ? <Notice variant="info">{add.notice ?? addedNotice}</Notice> : null}
        <Button type="button" variant="outline" className="w-full sm:w-auto sm:self-start" disabled={add.busy || !supported} aria-busy={add.busy} onClick={() => void onAdd()}>
          <Icon name="add" size={18} />
          Add a passkey
        </Button>
      </div>

      <Dialog open={renaming !== null} onOpenChange={(next) => (rename.busy ? undefined : !next && setRenaming(null))}>
        <DialogContent>
          <form
            className="grid gap-4"
            onSubmit={(event) => {
              event.preventDefault();
              void onRename();
            }}
          >
            <DialogHeader>
              <DialogTitle>Rename passkey</DialogTitle>
              <DialogDescription>Pick a name you will recognise, such as the device it lives on.</DialogDescription>
            </DialogHeader>
            <Field label="Passkey name" value={name} maxLength={64} onChange={(event) => setName(event.target.value)} />
            {rename.error ? <Notice variant="error">{rename.error}</Notice> : null}
            {rename.notice ? <Notice variant="info">{rename.notice}</Notice> : null}
            <DialogFooter>
              <Button type="button" variant="ghost" disabled={rename.busy} onClick={() => setRenaming(null)}>
                Cancel
              </Button>
              <Button type="submit" disabled={rename.busy || name.trim().length === 0 || name.trim() === renaming?.name} aria-busy={rename.busy}>
                Save
              </Button>
            </DialogFooter>
          </form>
        </DialogContent>
      </Dialog>

      <ConfirmDialog
        open={removing !== null}
        onOpenChange={(next) => !next && setRemoving(null)}
        title="Remove this passkey?"
        description={removing ? `${removing.name} can no longer sign in to your account. You will be asked to confirm with a passkey.` : ""}
        confirmLabel="Remove passkey"
        destructive
        busy={remove.busy}
        error={remove.error}
        notice={remove.notice}
        onConfirm={() => void onRemove()}
      />
    </>
  );
}
