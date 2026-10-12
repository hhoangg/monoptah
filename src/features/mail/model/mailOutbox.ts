import { useCallback, useEffect, useRef, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import { MAIL_OUTBOX_CHANGED_EVENT, mailOutboxList } from "./mail";
import type { OutboxRow, OutboxState } from "./mailTypes";

export type OutboxTone = "neutral" | "danger" | "caution";

export type OutboxPresentation = {
  label: string;
  tone: OutboxTone;
  /** The button that opens the row in the composer. */
  openLabel: string;
  canOpen: boolean;
  canDelete: boolean;
  /** Deleting forgets a message that may already have been delivered. */
  confirmDelete: boolean;
  hint: string | null;
};

/**
 * What the list shows and offers for a row. It reads `OutboxRow.state` and
 * nothing else: a `SendError.retrySafe` describes one call, not the row, and
 * is true for the refusal of an `unconfirmed` row.
 */
export function presentOutbox(state: OutboxState): OutboxPresentation {
  switch (state) {
    case "unconfirmed":
      return {
        label: "May have been sent",
        tone: "caution",
        openLabel: "Check recipient",
        canOpen: true,
        canDelete: true,
        confirmDelete: true,
        hint: "The connection dropped mid-send. Check the recipient before sending again.",
      };
    case "failed":
      return {
        label: "Failed",
        tone: "danger",
        openLabel: "Review",
        canOpen: true,
        canDelete: true,
        confirmDelete: false,
        hint: null,
      };
    case "sending":
      return {
        label: "Sending",
        tone: "neutral",
        openLabel: "Open",
        canOpen: false,
        canDelete: false,
        confirmDelete: false,
        hint: "Being sent now.",
      };
    case "queued":
      return {
        label: "Queued",
        tone: "neutral",
        openLabel: "Open",
        canOpen: true,
        canDelete: true,
        confirmDelete: false,
        hint: null,
      };
    case "sent":
      // Sent mail lives in its thread; the outbox list does not show it.
      return {
        label: "Sent",
        tone: "neutral",
        openLabel: "Open",
        canOpen: false,
        canDelete: true,
        confirmDelete: false,
        hint: null,
      };
    case "draft":
    default:
      return {
        label: "Draft",
        tone: "neutral",
        openLabel: "Edit",
        canOpen: true,
        canDelete: true,
        confirmDelete: false,
        hint: null,
      };
  }
}

/** Unconfirmed first, then failed, then the rest, each newest first. */
const ORDER: Record<OutboxState, number> = {
  unconfirmed: 0,
  failed: 1,
  sending: 2,
  queued: 3,
  draft: 4,
  sent: 5,
};

export function visibleOutbox(rows: OutboxRow[]): OutboxRow[] {
  return rows
    .filter((r) => r.state !== "sent")
    .sort(
      (a, b) => ORDER[a.state] - ORDER[b.state] || b.updatedAt - a.updatedAt,
    );
}

/**
 * The outbox rows, reloaded when one is sent, re-armed or deleted. It does not
 * listen to the messages event, and a save emits neither, so the composer's
 * autosave cannot reach it; the caller reloads when the composer closes.
 * That covers this window only: a draft saved by the composer in another
 * window leaves this list stale until its next reload (`mailOutboxSave` has no
 * caller outside the composer, so that is the only other way a row appears).
 */
export function useMailOutbox() {
  const [rows, setRows] = useState<OutboxRow[] | null>(null);
  const [error, setError] = useState<string | null>(null);
  const seq = useRef(0);
  const reload = useCallback(async () => {
    const mine = ++seq.current;
    try {
      const list = await mailOutboxList();
      if (mine !== seq.current) return;
      setRows(visibleOutbox(list));
      setError(null);
    } catch (err) {
      if (mine !== seq.current) return;
      setError(err instanceof Error ? err.message : String(err));
    }
  }, []);

  useEffect(() => {
    let disposed = false;
    let stop: (() => void) | null = null;
    void reload();
    void listen(MAIL_OUTBOX_CHANGED_EVENT, () => void reload())
      .then((off) => {
        if (disposed) off();
        else stop = off;
      })
      .catch(() => undefined);
    return () => {
      disposed = true;
      stop?.();
    };
  }, [reload]);

  return { rows, error, reload };
}
