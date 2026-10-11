import { useState } from "react";
import { Loader, Trash2 } from "../../../shared/ui/icons";
import { Modal } from "../../../shared/ui/Modal";
import { SecondaryButton } from "../../../shared/ui/SecondaryButton";
import { mailOutboxDelete } from "../model/mail";
import { presentOutbox, type OutboxTone } from "../model/mailOutbox";
import { addressLabel } from "../model/mailReply";
import type { OutboxRow } from "../model/mailTypes";

const TONE: Record<OutboxTone, string> = {
  neutral: "border-content/15 text-content/60",
  danger: "border-red-400/40 text-red-400",
  caution: "border-amber-400/60 bg-amber-400/10 text-amber-300",
};

type Props = {
  rows: OutboxRow[] | null;
  error: string | null;
  onOpen: (row: OutboxRow) => void;
  onChanged: () => void;
  onClose: () => void;
};

/**
 * Drafts and unsent messages. An `unconfirmed` row sorts first and opens the
 * composer's "check the recipient" path; nothing here sends.
 */
export function OutboxSheet({
  rows,
  error,
  onOpen,
  onChanged,
  onClose,
}: Props) {
  const [confirming, setConfirming] = useState<number | null>(null);
  const [busy, setBusy] = useState<number | null>(null);
  const [notice, setNotice] = useState<string | null>(null);

  const remove = async (row: OutboxRow) => {
    setBusy(row.id);
    setNotice(null);
    try {
      await mailOutboxDelete(row.id);
      setConfirming(null);
    } catch (err) {
      // Rust words this refusal for a person; show it as it is.
      setNotice(err instanceof Error ? err.message : String(err));
    } finally {
      setBusy(null);
      onChanged();
    }
  };

  return (
    <Modal
      title="Outbox"
      description="Drafts and messages that have not gone out."
      onClose={onClose}
      fitViewport
    >
      <div className="flex flex-col pb-2" data-mail-outbox>
        {notice ? (
          <p role="alert" className="px-4 py-2 text-[12px] text-red-400/90">
            {notice}
          </p>
        ) : null}
        {error ? (
          <p role="alert" className="px-4 py-2 text-[12px] text-red-400/90">
            Could not load the outbox: {error}
          </p>
        ) : null}
        {rows === null && !error ? (
          <div className="flex justify-center py-8 text-content/45">
            <Loader className="size-4 animate-spin" />
          </div>
        ) : null}
        {rows?.length === 0 ? (
          <p className="px-4 py-6 text-center text-[12px] text-content/45">
            Nothing waiting to be sent.
          </p>
        ) : null}
        <ul>
          {rows?.map((row) => {
            const p = presentOutbox(row.state);
            const to = row.to.map(addressLabel).join(", ");
            return (
              <li
                key={row.id}
                data-outbox-row={row.state}
                className="border-b border-stroke px-4 py-2.5"
              >
                <div className="flex items-center gap-2">
                  <span
                    className={`shrink-0 rounded border px-1.5 py-px text-[10px] ${TONE[p.tone]}`}
                  >
                    {p.label}
                  </span>
                  <span className="min-w-0 flex-1 truncate text-[13px] text-content">
                    {row.subject || "(no subject)"}
                  </span>
                </div>
                <p className="mt-0.5 truncate text-[11px] text-content/50">
                  {to ? `To ${to}` : "No recipients yet"}
                </p>
                {p.hint ? (
                  <p
                    className={`mt-1 text-[11px] ${
                      p.tone === "caution"
                        ? "text-amber-300"
                        : "text-content/50"
                    }`}
                  >
                    {p.hint}
                  </p>
                ) : null}
                {row.state === "failed" && row.lastError ? (
                  <p className="mt-1 text-[11px] text-red-400/90">
                    {row.lastError}
                  </p>
                ) : null}
                <div className="mt-2 flex items-center gap-2">
                  {p.canOpen ? (
                    <SecondaryButton onClick={() => onOpen(row)}>
                      {p.openLabel}
                    </SecondaryButton>
                  ) : null}
                  {confirming === row.id ? (
                    <>
                      <span className="text-[11px] text-content/60">
                        It may already be delivered. Forget it?
                      </span>
                      <SecondaryButton
                        disabled={busy === row.id}
                        onClick={() => void remove(row)}
                      >
                        Delete anyway
                      </SecondaryButton>
                      <SecondaryButton onClick={() => setConfirming(null)}>
                        Keep
                      </SecondaryButton>
                    </>
                  ) : (
                    <SecondaryButton
                      aria-label={`Delete ${row.subject || "message"}`}
                      disabled={!p.canDelete || busy === row.id}
                      title={p.canDelete ? undefined : "It is being sent"}
                      onClick={() =>
                        p.confirmDelete
                          ? setConfirming(row.id)
                          : void remove(row)
                      }
                    >
                      <Trash2 className="size-3" />
                      Delete
                    </SecondaryButton>
                  )}
                </div>
              </li>
            );
          })}
        </ul>
      </div>
    </Modal>
  );
}
