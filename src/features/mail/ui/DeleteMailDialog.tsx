import { useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { Archive, Loader, Trash2 } from "../../../shared/ui/icons";
import { Modal } from "../../../shared/ui/Modal";
import { SecondaryButton } from "../../../shared/ui/SecondaryButton";

/** What `mail_delete` returns. `kept` messages are still stored, on S3 too. */
export type DeleteReport = {
  removed: number;
  kept: number;
  threadsRemoved: number;
  /** Cached attachment folders that could not be removed. */
  filesLeft: number;
  errors: string[];
};

/**
 * `hard: true` is the only call that touches S3. It lives here rather than in
 * the model layer so the dialog and the one flag that decides between archive
 * and delete are read together.
 */
export const mailDelete = (threadIds: number[], hard: boolean) =>
  invoke<DeleteReport>("mail_delete", { threadIds, hard });

/** Typed to confirm a delete on S3. */
export const CONFIRM_WORD = "delete";

type Choice = "archive" | "hard";

export function DeleteMailDialog({
  threadIds,
  subject,
  messageCount,
  mailbox,
  onClose,
  onDone,
}: {
  threadIds: number[];
  subject: string;
  messageCount: number;
  mailbox: string;
  onClose: () => void;
  /** Called when the work finished, so the caller can drop what it shows. */
  onDone: (report: DeleteReport, hard: boolean) => void;
}) {
  // Archive is the default and a plain Enter cannot reach the other choice.
  const [choice, setChoice] = useState<Choice>("archive");
  const [typed, setTyped] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [partial, setPartial] = useState<DeleteReport | null>(null);

  const hard = choice === "hard";
  const armed = !hard || typed.trim().toLowerCase() === CONFIRM_WORD;
  const noun = threadIds.length === 1 ? "thread" : "threads";

  const submit = async () => {
    if (busy || !armed) return;
    setBusy(true);
    setError(null);
    try {
      const report = await mailDelete(threadIds, hard);
      onDone(report, hard);
      // Anything S3 refused stays in the mailbox. Say so instead of closing.
      if (report.kept > 0 || report.errors.length > 0 || report.filesLeft > 0) {
        setPartial(report);
        setBusy(false);
      } else {
        onClose();
      }
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
      setBusy(false);
    }
  };

  if (partial) {
    return (
      <Modal
        title={
          partial.kept > 0
            ? "Some mail was not deleted"
            : "Deleted, with files left behind"
        }
        size="sm"
        onClose={onClose}
      >
        <div className="flex flex-col gap-2 px-4 py-3 text-[12px] text-content/70">
          <p role="alert">
            {partial.removed} deleted.
            {partial.kept > 0
              ? ` ${partial.kept} could not be deleted from S3, so ${
                  partial.kept === 1 ? "it is" : "they are"
                } still in your mailbox.`
              : ""}
          </p>
          {partial.filesLeft > 0 ? (
            <p>
              The mail is gone, but downloaded attachment files for{" "}
              {partial.filesLeft}{" "}
              {partial.filesLeft === 1 ? "message" : "messages"} could not be
              removed from this computer.
            </p>
          ) : null}
          {partial.errors.map((e) => (
            // S3's error code in Monoptah's words. A failed batch can blame
            // keys it never sent, so this says what went wrong, not which
            // message caused it.
            <p key={e} className="text-red-400">
              {e}
            </p>
          ))}
        </div>
        <div className="flex justify-end px-4 pb-3">
          <SecondaryButton onClick={onClose}>Close</SecondaryButton>
        </div>
      </Modal>
    );
  }

  return (
    <Modal
      title={`Delete ${threadIds.length === 1 ? "1 thread" : `${threadIds.length} threads`}?`}
      description={subject ? `${subject} · ${mailbox}` : mailbox}
      size="md"
      onClose={onClose}
    >
      <form
        className="flex flex-col gap-2 px-4 pt-3 pb-3"
        onSubmit={(e) => {
          e.preventDefault();
          void submit();
        }}
      >
        <div role="radiogroup" aria-label="What to do" className="contents">
          <label
            className={`flex cursor-pointer gap-2.5 rounded-lg border p-2.5 ${
              !hard
                ? "border-accent/40 bg-accent/10"
                : "border-content/10 hover:bg-content/5"
            }`}
          >
            <input
              type="radio"
              name="delete-choice"
              checked={!hard}
              onChange={() => setChoice("archive")}
              className="mt-0.5"
            />
            <span className="flex flex-col gap-1">
              <span className="flex items-center gap-1.5 text-[12px] font-semibold text-content">
                <Archive className="size-3.5" />
                Archive locally
              </span>
              <span className="text-[11.5px] leading-snug text-content/50">
                Removes the {noun} from your mailboxes and keeps the S3 objects.
                It stays under Archive in the folder list, where you can read it
                or move it back to the inbox.
              </span>
            </span>
          </label>

          <label
            className={`flex cursor-pointer gap-2.5 rounded-lg border p-2.5 ${
              hard
                ? "border-red-400/50 bg-red-400/10"
                : "border-red-400/20 hover:bg-content/5"
            }`}
          >
            <input
              type="radio"
              name="delete-choice"
              checked={hard}
              onChange={() => setChoice("hard")}
              className="mt-0.5"
            />
            <span className="flex min-w-0 flex-1 flex-col gap-1">
              <span className="flex items-center gap-1.5 text-[12px] font-semibold text-red-400">
                <Trash2 className="size-3.5" />
                Delete on S3 as well
              </span>
              <span className="text-[11.5px] leading-snug text-content/50">
                Deletes the S3 object of each message that has one, and removes
                the messages from Monoptah ({messageCount} in this {noun}). A
                message sent from Monoptah has no S3 object and is only removed
                here. This cannot be undone: the S3 object is the only copy of
                the original message unless versioning is turned on for the
                bucket, which Monoptah cannot check.
              </span>
            </span>
          </label>
        </div>

        {hard ? (
          <div className="flex flex-col gap-1 pt-1">
            <label
              htmlFor="delete-confirm"
              className="text-[11.5px] text-content/60"
            >
              Type <b className="text-content">{CONFIRM_WORD}</b> to confirm
            </label>
            <input
              id="delete-confirm"
              autoFocus
              autoComplete="off"
              spellCheck={false}
              value={typed}
              onChange={(e) => setTyped(e.target.value)}
              className="rounded-md border border-content/10 bg-transparent px-2 py-1 text-[12px] text-content focus-visible:outline-2 focus-visible:outline-accent"
            />
          </div>
        ) : null}

        {error ? (
          <p role="alert" className="text-[12px] text-red-400">
            {error}
          </p>
        ) : null}

        <div className="flex items-center justify-end gap-2 pt-2">
          <SecondaryButton onClick={onClose} disabled={busy}>
            Cancel
          </SecondaryButton>
          <SecondaryButton
            type="submit"
            danger={hard}
            disabled={busy || !armed}
          >
            {busy ? (
              <Loader className="size-3 animate-spin" />
            ) : hard ? (
              <Trash2 className="size-3" />
            ) : (
              <Archive className="size-3" />
            )}
            {hard ? "Delete from S3" : "Archive"}
          </SecondaryButton>
        </div>
      </form>
    </Modal>
  );
}
