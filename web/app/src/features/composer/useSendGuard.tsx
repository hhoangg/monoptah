// The `send-guard` seam (workstreams.md 3.2) in front of every send and start: an async hook that may allow, ask
// the user to confirm a notice (a cost notice for a stopped cloud instance) or refuse with a reason. With no hook
// registered it allows. `check` resolves when the send may go on and rejects with `SendRefused` (show the reason)
// or `SendCancelled` (the user said no; nothing to show). `dialog` must be rendered by the caller.
import { useCallback, useRef, useState, type ReactElement } from "react";
import type { SendOptions, SendResult } from "@/data";
import { WAKE_REQUIRED_NOTICE, runSendGuard, type SendGuardContext } from "@/ui/hooks";
import { ConfirmDialog } from "@/ui/ConfirmDialog";
import { sendErrorMessage } from "./sendErrors";

export class SendRefused extends Error {
  override name = "SendRefused";
}

export class SendCancelled extends Error {
  override name = "SendCancelled";
}

/** What `check` found out: `wake` is true when the user confirmed a notice that asks to wake a stopped instance. */
export interface SendCheck {
  wake: boolean;
}

export function useSendGuard(): {
  check: (context: SendGuardContext) => Promise<SendCheck>;
  /** Shows a notice and resolves when the user confirms; rejects with `SendCancelled` otherwise. */
  confirm: (notice: string) => Promise<void>;
  /**
   * Guards and sends: `run` is called with `{wake: true}` only after the user confirmed a wake notice. When the
   * desktop still answers `wake_required` (the cached state said available) the wake notice is shown and `run` is
   * called once more with `wake: true` if the user confirms, exactly once per confirmation. Rejects with
   * `SendRefused` or `SendCancelled`. A second `wake_required` is returned as the failed result.
   */
  guardedSend: (context: SendGuardContext, run: (options: SendOptions) => Promise<SendResult>) => Promise<SendResult>;
  dialog: ReactElement;
} {
  const [notice, setNotice] = useState<string | null>(null);
  const answer = useRef<((ok: boolean) => void) | null>(null);

  const settle = (ok: boolean) => {
    const resolve = answer.current;
    answer.current = null;
    setNotice(null);
    resolve?.(ok);
  };

  const confirm = useCallback(async (text: string) => {
    const ok = await new Promise<boolean>((resolve) => {
      answer.current = resolve;
      setNotice(text);
    });
    if (!ok) throw new SendCancelled("Send cancelled");
  }, []);

  const check = useCallback(
    async (context: SendGuardContext): Promise<SendCheck> => {
      const result = await runSendGuard(context);
      if (result.kind === "refuse") throw new SendRefused(result.reason);
      if (result.kind === "confirm") {
        await confirm(result.notice);
        return { wake: result.wake === true };
      }
      return { wake: false };
    },
    [confirm],
  );

  const guardedSend = useCallback(
    async (context: SendGuardContext, run: (options: SendOptions) => Promise<SendResult>): Promise<SendResult> => {
      const { wake } = await check(context);
      const first = await run(wake ? { wake: true } : {});
      if (first.sent || first.error !== "wake_required" || wake) return first;
      await confirm(WAKE_REQUIRED_NOTICE);
      return run({ wake: true });
    },
    [check, confirm],
  );

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
  return { check, confirm, guardedSend, dialog };
}

export interface GuardedRepliesOptions {
  desktopId: string;
  sessionId: string;
  target: SendGuardContext["target"];
  send: (text: string, attachments?: undefined, options?: SendOptions) => Promise<SendResult>;
  retry: (optimisticId: string, options?: SendOptions) => Promise<SendResult>;
}

/**
 * The two sends that do not start in the composer, "Not sent, retry" and a tapped answer choice, run through the
 * same `send-guard` as a typed message: it may confirm (the dialog), refuse with a reason (`error`) or allow.
 * `dialog` and `error` must be rendered by the caller; call `clearError` when a later send goes out. A user cancel of the
 * confirm rejects `onChoose` with `SendCancelled`, which the choice cards treat as silent.
 */
export function useGuardedReplies({ desktopId, sessionId, target, send, retry }: GuardedRepliesOptions) {
  const guard = useSendGuard();
  const { guardedSend } = guard;
  const [error, setError] = useState<string | null>(null);

  const onChoose = useCallback(
    async (text: string) => {
      const result = await guardedSend({ desktopId, sessionId, target }, (options) => send(text, undefined, options));
      if (!result.sent) throw new Error(sendErrorMessage(result.error));
    },
    [guardedSend, desktopId, sessionId, target, send],
  );

  const onRetry = useCallback(
    (optimisticId: string) => {
      setError(null);
      void (async () => {
        try {
          await guardedSend({ desktopId, sessionId, target }, (options) => retry(optimisticId, options));
        } catch (reason) {
          if (reason instanceof SendRefused) setError(reason.message);
        }
      })();
    },
    [guardedSend, desktopId, sessionId, target, retry],
  );

  const clearError = useCallback(() => setError(null), []);
  return { onChoose, onRetry, dialog: guard.dialog, error, clearError };
}
