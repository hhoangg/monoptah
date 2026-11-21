// The question before a terminal tab (or a pane of them) closes while its shell is running. Closing kills the shell.
import { ConfirmDialog } from "@/ui/ConfirmDialog";
import { setCloseRequest, useCloseRequest } from "./sessions";

export function TerminalCloseConfirm() {
  const request = useCloseRequest();
  return (
    <ConfirmDialog
      open={request !== null}
      onOpenChange={(open) => (open ? undefined : setCloseRequest(null))}
      title={request && request.count > 1 ? `Close ${request.count} terminals?` : "Close this terminal?"}
      description="This ends the shell on your desktop and everything running in it."
      confirmLabel="Close terminal"
      destructive
      onConfirm={() => request?.confirm()}
    />
  );
}
