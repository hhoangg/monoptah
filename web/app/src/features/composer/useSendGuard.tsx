// The `send-guard` seam (workstreams.md 3.2) in front of every send and start: an async hook that may allow, ask
// the user to confirm a notice (a cost notice for a stopped cloud instance) or refuse with a reason. With no hook
// registered it allows. `check` resolves when the send may go on and rejects with `SendRefused` (show the reason)
// or `SendCancelled` (the user said no; nothing to show). `dialog` must be rendered by the caller.
import { useCallback, useRef, useState, type ReactElement } from "react";
import { runSendGuard, type SendGuardContext } from "@/ui/hooks";
import { ConfirmDialog } from "@/ui/ConfirmDialog";

export class SendRefused extends Error {
  override name = "SendRefused";
}

export class SendCancelled extends Error {
  override name = "SendCancelled";
}

export function useSendGuard(): { check: (context: SendGuardContext) => Promise<void>; dialog: ReactElement } {
  const [notice, setNotice] = useState<string | null>(null);
  const answer = useRef<((ok: boolean) => void) | null>(null);

  const settle = (ok: boolean) => {
    const resolve = answer.current;
    answer.current = null;
    setNotice(null);
    resolve?.(ok);
  };

  const check = useCallback(async (context: SendGuardContext) => {
    const result = await runSendGuard(context);
    if (result.kind === "refuse") throw new SendRefused(result.reason);
    if (result.kind === "confirm") {
      const ok = await new Promise<boolean>((resolve) => {
        answer.current = resolve;
        setNotice(result.notice);
      });
      if (!ok) throw new SendCancelled("Send cancelled");
    }
  }, []);

  const dialog = (
    <ConfirmDialog
      open={notice !== null}
      onOpenChange={(open) => {
        if (!open) settle(false);
      }}
      title="Send this message?"
      description={notice ?? ""}
      confirmLabel="Send"
      onConfirm={() => settle(true)}
    />
  );
  return { check, dialog };
}
