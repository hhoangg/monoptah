import { useCallback, useRef, useState } from "react";
import { ReauthCancelledError } from "@/features/accounts/queries";
import { REAUTH_CANCELLED, settingsErrorMessage } from "./copy";

export interface ActionState {
  busy: boolean;
  /** Failure text of the last run. */
  error: string | null;
  /** Soft message: the user closed the passkey prompt. Not an error. */
  notice: string | null;
}

/**
 * Runs one async action at a time and tracks busy, error and the soft "prompt closed" notice.
 * `run` resolves to true when the action finished. Results (codes, invite codes) must be kept by
 * the caller in its own state: nothing here stores them.
 */
export function useAction() {
  const [state, setState] = useState<ActionState>({ busy: false, error: null, notice: null });
  const running = useRef(false);

  const run = useCallback(async (action: () => Promise<unknown>): Promise<boolean> => {
    if (running.current) return false;
    running.current = true;
    setState({ busy: true, error: null, notice: null });
    try {
      await action();
      setState({ busy: false, error: null, notice: null });
      return true;
    } catch (caught) {
      if (caught instanceof ReauthCancelledError) setState({ busy: false, error: null, notice: REAUTH_CANCELLED });
      else setState({ busy: false, error: settingsErrorMessage(caught), notice: null });
      return false;
    } finally {
      running.current = false;
    }
  }, []);

  const reset = useCallback(() => setState({ busy: false, error: null, notice: null }), []);
  return { ...state, run, reset };
}
