import { useRef, useState, type ReactNode, type RefObject } from "react";
import { Modal } from "../../../shared/ui/Modal";

/** An extra choice offered in the dialog, such as also deleting files. */
export type ConfirmResetOption = {
  label: string;
  /** Said under the label, such as why the choice is unavailable. */
  note?: string;
  /** Unavailable: shown unchecked and cannot be changed. */
  disabled?: boolean;
  /** Checked when the dialog opens. Defaults to true. */
  defaultChecked?: boolean;
};

const startsChecked = (option: ConfirmResetOption | undefined) =>
  !!option && !option.disabled && option.defaultChecked !== false;

/**
 * A destructive reset that asks first, then reports a failure in place. A
 * message returned from `onConfirm` closes the dialog and shows beside the
 * trigger, for trouble that must not undo the reset.
 */
export function ConfirmReset({
  label,
  title,
  body,
  kept,
  failure,
  option,
  onConfirm,
  children,
}: {
  /** The confirm button, and what the trigger says. */
  label: string;
  title: string;
  body: string;
  /** What survives, said separately so it reads as reassurance. */
  kept: string;
  failure: string;
  option?: ConfirmResetOption;
  /** Receives whether the option is ticked; false when there is none. */
  onConfirm: (checked: boolean) => Promise<string | void>;
  /** The trigger; focus returns to `ref` when the dialog closes. */
  children: (
    open: () => void,
    ref: RefObject<HTMLButtonElement | null>,
  ) => ReactNode;
}) {
  const [confirming, setConfirming] = useState(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string>();
  const [notice, setNotice] = useState<string>();
  // Until the user touches the box, its value follows the option's props.
  const [touched, setTouched] = useState<boolean>();
  const checked = option?.disabled
    ? false
    : (touched ?? startsChecked(option));
  const running = useRef(false);
  const trigger = useRef<HTMLButtonElement>(null);
  const close = () => {
    if (running.current) return;
    setConfirming(false);
    trigger.current?.focus();
  };
  const reset = async () => {
    if (running.current) return;
    running.current = true;
    setBusy(true);
    setError(undefined);
    try {
      const message = await onConfirm(checked);
      setNotice(typeof message === "string" && message ? message : undefined);
      setConfirming(false);
    } catch (failure) {
      setError(failure instanceof Error ? failure.message : String(failure));
    } finally {
      running.current = false;
      setBusy(false);
    }
  };
  return (
    <>
      {children(() => {
        setError(undefined);
        setNotice(undefined);
        setTouched(undefined);
        setConfirming(true);
      }, trigger)}
      {notice ? (
        <p
          role="status"
          className="px-1 pt-2 text-[12px] leading-5 text-amber-300"
        >
          {notice}
        </p>
      ) : null}
      {confirming ? (
        <Modal title={title} size="sm" onClose={close}>
          <div className="flex flex-col gap-3 px-4 pb-4 pt-3">
            <p className="text-[13px] leading-5 text-content/70">{body}</p>
            <p className="text-[12px] leading-5 text-content/50">{kept}</p>
            {option ? (
              <label className="flex items-start gap-2 text-[12px]">
                <input
                  type="checkbox"
                  checked={checked}
                  disabled={option.disabled || busy}
                  onChange={(e) => setTouched(e.target.checked)}
                  className="mt-0.5 accent-accent"
                />
                <span
                  className={option.disabled ? "text-content/50" : undefined}
                >
                  {option.label}
                  {option.note ? (
                    <span className="mt-1 block text-[11px] text-content/45">
                      {option.note}
                    </span>
                  ) : null}
                </span>
              </label>
            ) : null}
            {error ? (
              <p role="alert" className="text-[12px] leading-5 text-red-400">
                {failure} {error}
              </p>
            ) : null}
            <div className="flex justify-end gap-2 pt-1">
              <button
                type="button"
                disabled={busy}
                onClick={close}
                className="rounded-md px-3 py-1.5 text-[12px] text-content/70 enabled:hover:bg-content/8 disabled:opacity-50"
              >
                Cancel
              </button>
              <button
                type="button"
                disabled={busy}
                onClick={() => void reset()}
                className="rounded-md bg-red-500/20 px-3 py-1.5 text-[12px] font-medium text-red-300 enabled:hover:bg-red-500/30 disabled:opacity-50"
              >
                {busy ? "Resetting…" : label}
              </button>
            </div>
          </div>
        </Modal>
      ) : null}
    </>
  );
}
