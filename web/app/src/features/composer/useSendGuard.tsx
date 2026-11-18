// The `send-guard` seam (workstreams.md 3.2) in front of every send and start: an async hook that may allow, ask
// the user to confirm a notice (a cost notice for a stopped cloud instance) or refuse with a reason. With no hook
// registered it allows. `check` resolves when the send may go on and rejects with `SendRefused` (show the reason)
// or `SendCancelled` (the user said no; nothing to show). `dialog` must be rendered by the caller.
import { useCallback, useRef, useState, type ReactElement } from "react";
import type { SendResult } from "@/data";
import { runSendGuard, type SendGuardContext } from "@/ui/hooks";
import { ConfirmDialog } from "@/ui/ConfirmDialog";
import { sendErrorMessage } from "./sendErrors";

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

export interface GuardedRepliesOptions {
  desktopId: string;
  sessionId: string;
  target: SendGuardContext["target"];
  send: (text: string) => Promise<SendResult>;
  retry: (optimisticId: string) => Promise<SendResult>;
}

/**
 * The two sends that do not start in the composer, "Not sent, retry" and a tapped answer choice, run through the
 * same `send-guard` as a typed message: it may confirm (the dialog), refuse with a reason (`error`) or allow.
 * `dialog` and `error` must be rendered by the caller; call `clearError` when a later send goes out. A user cancel of the
 * confirm rejects `onChoose` with `SendCancelled`, which the choice cards treat as silent.
 */
export function useGuardedReplies({ desktopId, sessionId, target, send, retry }: GuardedRepliesOptions) {
  const guard = useSendGuard();
  const { check } = guard;
  const [error, setError] = useState<string | null>(null);

  const onChoose = useCallback(
    async (text: string) => {
      await check({ desktopId, sessionId, target });
      const result = await send(text);
      if (!result.sent) throw new Error(sendErrorMessage(result.error));
    },
    [check, desktopId, sessionId, target, send],
  );

  const onRetry = useCallback(
    (optimisticId: string) => {
      setError(null);
      void (async () => {
        try {
          await check({ desktopId, sessionId, target });
        } catch (reason) {
          if (reason instanceof SendRefused) setError(reason.message);
          return;
        }
        await retry(optimisticId);
      })();
    },
    [check, desktopId, sessionId, target, retry],
  );

  const clearError = useCallback(() => setError(null), []);
  return { onChoose, onRetry, dialog: guard.dialog, error, clearError };
}
