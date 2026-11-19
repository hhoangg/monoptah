// The desktop rename form, shared by the detail screen (a card) and the /desktops list (a dialog).
import { useQueryClient } from "@tanstack/react-query";
import { useState, type FormEvent } from "react";
import { useAction } from "@/features/settings/useAction";
import { Button } from "@/ui/button";
import { Field } from "@/ui/Field";
import { Notice } from "@/ui/Notice";
import { renameDesktop } from "./api";
import { desktopErrorMessage } from "./parts";

export function RenameDesktopForm({ desktop, onSaved }: { desktop: { id: string; name: string }; onSaved?: () => void }) {
  const queryClient = useQueryClient();
  const action = useAction(desktopErrorMessage);
  const [draft, setDraft] = useState(desktop.name);
  // The name changed elsewhere (another tab or device, the 15 s refetch): follow it unless the field has edits.
  const [seenName, setSeenName] = useState(desktop.name);
  if (desktop.name !== seenName) {
    setSeenName(desktop.name);
    if (draft === seenName) setDraft(desktop.name);
  }
  const [saved, setSaved] = useState(false);
  const trimmed = draft.trim();
  const tooLong = [...trimmed].length > 64;
  const validation = trimmed === "" ? "Enter a name." : tooLong ? "Use 64 characters or fewer." : null;
  const unchanged = trimmed === desktop.name;

  async function submit(event: FormEvent) {
    event.preventDefault();
    if (validation || unchanged) return;
    setSaved(false);
    const ok = await action.run(() => renameDesktop(queryClient, desktop.id, trimmed));
    if (ok) {
      setDraft(trimmed);
      setSaved(true);
      onSaved?.();
    }
  }

  return (
    <form onSubmit={(event) => void submit(event)} className="flex flex-col gap-3 px-1.5 py-2" noValidate>
      <Field
        label="Desktop name"
        value={draft}
        maxLength={200}
        autoComplete="off"
        onChange={(event) => {
          setDraft(event.target.value);
          setSaved(false);
        }}
        error={draft !== desktop.name ? validation : null}
      />
      {action.error ? <Notice variant="error">{action.error}</Notice> : null}
      {saved ? <Notice variant="info">Name saved.</Notice> : null}
      <Button type="submit" variant="outline" className="w-full sm:w-auto sm:self-start" disabled={action.busy || validation !== null || unchanged} aria-busy={action.busy}>
        Save name
      </Button>
    </form>
  );
}
