import { useQueryClient } from "@tanstack/react-query";
import { useState } from "react";
import { updateDisplayName } from "@/features/accounts/queries";
import { Button } from "@/ui/button";
import { Dialog, DialogContent, DialogDescription, DialogFooter, DialogHeader, DialogTitle } from "@/ui/dialog";
import { Field } from "@/ui/Field";
import { Notice } from "@/ui/Notice";
import { SETTINGS_ANCHOR, SettingsCard, SettingsRow } from "@/ui/SettingsCard";
import { useAction } from "./useAction";

export interface AccountSectionProps {
  handle: string;
  displayName: string;
}

/** Display name (editable, no fresh auth) and handle (read only). */
export function AccountSection({ handle, displayName }: AccountSectionProps) {
  const queryClient = useQueryClient();
  const [open, setOpen] = useState(false);
  const [value, setValue] = useState(displayName);
  const action = useAction();
  const trimmed = value.trim();
  const valid = trimmed.length >= 1 && trimmed.length <= 64;

  async function save() {
    if (!valid) return;
    if (await action.run(() => updateDisplayName(queryClient, trimmed))) setOpen(false);
  }

  return (
    <SettingsCard title="Account" footnote="Your handle identifies you to the people you invite and cannot be changed.">
      <SettingsRow
        id="display-name"
        className={SETTINGS_ANCHOR}
        icon="people"
        title="Display name"
        description={displayName}
        trailing={
          <Button
            type="button"
            variant="outline"
            size="sm"
            onClick={() => {
              setValue(displayName);
              action.reset();
              setOpen(true);
            }}
          >
            Edit
            <span className="sr-only"> display name</span>
          </Button>
        }
      />
      <SettingsRow id="handle" className={SETTINGS_ANCHOR} icon="key" title="Handle" description={`@${handle}`} />
      <Dialog open={open} onOpenChange={(next) => (action.busy ? undefined : setOpen(next))}>
        <DialogContent>
          <form
            className="grid gap-4"
            onSubmit={(event) => {
              event.preventDefault();
              void save();
            }}
          >
            <DialogHeader>
              <DialogTitle>Edit display name</DialogTitle>
              <DialogDescription>Shown to the people you invite and in your own session list.</DialogDescription>
            </DialogHeader>
            <Field label="Display name" value={value} maxLength={64} autoComplete="name" onChange={(event) => setValue(event.target.value)} />
            {action.error ? <Notice variant="error">{action.error}</Notice> : null}
            <DialogFooter>
              <Button type="button" variant="ghost" disabled={action.busy} onClick={() => setOpen(false)}>
                Cancel
              </Button>
              <Button type="submit" disabled={!valid || action.busy || trimmed === displayName}>
                Save
              </Button>
            </DialogFooter>
          </form>
        </DialogContent>
      </Dialog>
    </SettingsCard>
  );
}
