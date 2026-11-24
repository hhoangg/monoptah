// Reset a Mono's conversation, as the desktop app's Mono settings do (wording from crates/monocode-gpui/src/monos/reset.rs).
// A web device must prove a fresh passkey first: each confirm mints a single-use ticket at the Worker (a passkey prompt
// when the server asks for one) and sends it with `mono.reset`. A retry is a new ticket, so a new command.
import { useQueryClient } from "@tanstack/react-query";
import { useState } from "react";
import { catalogKey, linkKey, useLinkMutation, useLinkQuery } from "@/data";
import { openConversation } from "@/data/conversation";
import { isApiError } from "@/features/accounts/api";
import { ReauthCancelledError } from "@/features/accounts/queries";
import { requestMonoResetTicket } from "@/features/desktops/api";
import { useThisDevice } from "@/features/desktops/thisDevice";
import { useReadOnly } from "@/features/sessions/WorkspaceFrame";
import { useLinkConnection } from "@/link";
import { useCapability } from "@/version";
import { Checkbox } from "@/ui/Checkbox";
import { ConfirmDialog } from "@/ui/ConfirmDialog";

/** What the dialog offers for the provider's own session files (`resetFiles` of `mono.profile`). */
export interface ResetFiles {
  label: string;
  supported: boolean;
  note?: string | undefined;
}

/** Whether this device may reset a conversation: the desktop advertises `monoReset` (only to Control devices) and the device is not read-only. */
export function useCanResetMono(): boolean {
  const capable = useCapability("monoReset");
  const readOnly = useReadOnly();
  return capable && !readOnly;
}

/** The line under the option, as the desktop app words it. */
export function resetFilesNote({ supported, note }: ResetFiles): string {
  if (!supported) return note ?? "";
  return note ? `They can't be recovered. ${note}` : "They can't be recovered.";
}

function failure(error: unknown): string {
  if (error instanceof ReauthCancelledError) return "Passkey confirmation was cancelled. Reset again to retry.";
  const code = (error as { code?: unknown } | null)?.code;
  if (isApiError(error, "rate_limited")) return "Too many reset attempts. Wait a few minutes and try again.";
  if (isApiError(error, "forbidden")) return "This device is not allowed to reset conversations.";
  const detail = code === "fresh_required" ? "Confirm with your passkey and try again." : error instanceof Error ? error.message : "";
  return `Could not reset the conversation.${detail ? ` ${detail}` : ""}`;
}

export interface ResetMonoDialogProps {
  desktopId: string;
  monoId: string;
  name: string;
  onOpenChange: (open: boolean) => void;
}

/** Mount it while it is open: its state (the checkbox, the last error) starts fresh each time. */
export function ResetMonoDialog({ desktopId, monoId, name, onOpenChange }: ResetMonoDialogProps) {
  const queryClient = useQueryClient();
  const connection = useLinkConnection();
  const device = useThisDevice();
  const reset = useLinkMutation("mono.reset");
  // Read afresh on open (the shared default never refetches): what the option offers must be what the desktop says now.
  const profile = useLinkQuery("mono.profile", { monoId }, { staleTime: 0, refetchOnMount: "always" });
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [deleteFiles, setDeleteFiles] = useState(true);
  // A failed read leaves the option out (and `deleteFiles` false), never an older answer.
  const files = profile.isError ? undefined : profile.data?.resetFiles;

  const confirm = async () => {
    setBusy(true);
    setError(null);
    try {
      if (!device?.deviceId) throw new Error("This browser is not registered with your account yet.");
      const ticket = await requestMonoResetTicket(desktopId, device.deviceId, monoId);
      // Provider files go only when the dialog showed the option, checked.
      await reset.mutateAsync({ monoId, ticket, deleteFiles: files ? files.supported && deleteFiles : false });
    } catch (reason) {
      setError(failure(reason));
      setBusy(false);
      return;
    }
    setBusy(false);
    onOpenChange(false);
    void queryClient.invalidateQueries({ queryKey: catalogKey(desktopId) });
    void queryClient.invalidateQueries({ queryKey: linkKey(desktopId, "mono.profile", { monoId }) });
    // The chat shows the empty conversation without waiting for a watch frame.
    const handle = openConversation(queryClient, connection, `mono:${monoId}`);
    void handle.watcher.read().catch(() => undefined).finally(() => handle.close());
  };

  return (
    <ConfirmDialog
      open
      onOpenChange={onOpenChange}
      title={`Reset ${name}'s conversation?`}
      description={
        <>
          <span className="block">All messages in this Mono's conversation will be deleted and any active reply will be stopped. This can't be undone.</span>
          <span className="mt-2 block">Its soul, memory and habits will be kept.</span>
          {files ? (
            <span className="mt-3 block">
              <Checkbox
                label={`Also delete the ${files.label} session files on this computer`}
                checked={files.supported && deleteFiles}
                disabled={!files.supported || busy}
                onChange={(event) => setDeleteFiles(event.target.checked)}
                className="min-h-9"
              />
              {resetFilesNote(files) ? <span className="mt-0.5 block pl-[30px] text-small text-muted-text">{resetFilesNote(files)}</span> : null}
            </span>
          ) : null}
        </>
      }
      confirmLabel={busy ? "Resetting…" : "Reset conversation"}
      destructive
      busy={busy}
      confirmDisabled={profile.isFetching}
      error={error}
      onConfirm={() => void confirm()}
    />
  );
}
