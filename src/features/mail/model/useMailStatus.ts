import { useCallback, useEffect, useRef, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import {
  MAIL_MESSAGES_CHANGED_EVENT,
  MAIL_SYNC_STATE_EVENT,
  mailStatus,
} from "./mail";
import type { MailStatus, SyncStateEvent } from "./mailTypes";

/** Mailboxes that are syncing right now, keyed by mailbox id. */
export type RunningSyncs = ReadonlyMap<
  number,
  { accountId: number; done: number; total: number }
>;

/**
 * Folds one `mail://sync-state` event in. A mailbox's own `done`/`error` ends
 * it; the source-level event (no mailbox) is sent after all of the source's
 * mailboxes and also clears any a failed listing left behind.
 */
export function applySyncEvent(
  running: RunningSyncs,
  event: SyncStateEvent,
): RunningSyncs {
  const next = new Map(running);
  if (event.mailboxId === null) {
    for (const [id, entry] of running) {
      if (entry.accountId === event.accountId) next.delete(id);
    }
  } else if (event.phase === "running") {
    next.set(event.mailboxId, {
      accountId: event.accountId,
      done: event.done,
      total: event.total,
    });
  } else {
    next.delete(event.mailboxId);
  }
  return next;
}

/** "42 of 214" across every running mailbox; `total` 0 while still listing. */
export type SyncProgress = { done: number; total: number };

export function syncProgress(running: RunningSyncs): SyncProgress | null {
  if (running.size === 0) return null;
  let done = 0;
  let total = 0;
  for (const entry of running.values()) {
    done += entry.done;
    total += entry.total;
  }
  return { done, total };
}

/**
 * Status for the whole Mail view, and the progress of a sync that is running.
 *
 * Rust reports one sync-state event per object fetched. Those carry progress,
 * not a change to anything `mail_status` returns, so only the end of a mailbox
 * or source (and any change of messages) asks for a read. Reads are one at a
 * time with one queued, as in `useMailUnread`: every read walks all mailboxes
 * under the app-wide store lock. Because only one is ever in flight, results
 * cannot land out of order, and a read that was queued starts after the one
 * before it has been applied, so the newest state always wins.
 */
export function useMailStatus() {
  const [status, setStatus] = useState<MailStatus | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [running, setRunning] = useState<RunningSyncs>(new Map());
  const reading = useRef<Promise<void> | null>(null);
  const queued = useRef(false);
  const alive = useRef(true);

  const refresh = useCallback((): Promise<void> => {
    if (reading.current) {
      queued.current = true;
      return reading.current;
    }
    const run = (async () => {
      try {
        const next = await mailStatus();
        if (alive.current) {
          setStatus(next);
          setError(null);
        }
      } catch (err) {
        if (alive.current)
          setError(err instanceof Error ? err.message : String(err));
      }
    })().then(() => {
      reading.current = null;
      if (queued.current && alive.current) {
        queued.current = false;
        return refresh();
      }
    });
    reading.current = run;
    return run;
  }, []);

  useEffect(() => {
    alive.current = true;
    let disposed = false;
    const stops: (() => void)[] = [];
    void refresh();
    const subscribe = (
      event: string,
      handler: (payload: SyncStateEvent) => void,
    ) =>
      listen<SyncStateEvent>(event, (e) => handler(e.payload))
        .then((stop) => {
          if (disposed) stop();
          else stops.push(stop);
        })
        .catch(() => undefined);
    void subscribe(MAIL_SYNC_STATE_EVENT, (payload) => {
      setRunning((current) => applySyncEvent(current, payload));
      if (payload.phase !== "running") void refresh();
    });
    void subscribe(MAIL_MESSAGES_CHANGED_EVENT, () => void refresh());
    return () => {
      disposed = true;
      alive.current = false;
      for (const stop of stops) stop();
    };
  }, [refresh]);

  return { status, error, refresh, progress: syncProgress(running) };
}
