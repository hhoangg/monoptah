import { useEffect, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import {
  MAIL_MESSAGES_CHANGED_EVENT,
  MAIL_SYNC_STATE_EVENT,
  mailStatus,
} from "./mail";

/**
 * The rail's unread count: `unreadTotal` from `mail_status`, which Rust sums
 * over enabled sources only, so a source switched off stops counting here.
 * Reads nothing while Mail is off, so a user without it pays no sync traffic.
 */
export function useMailUnread(enabled: boolean): number {
  const [unread, setUnread] = useState(0);

  useEffect(() => {
    if (!enabled) {
      setUnread(0);
      return;
    }
    let disposed = false;
    // One read at a time: a sync reports progress many times, and every read
    // walks all mailboxes. Events that land mid-read cost one more read.
    let reading = false;
    let again = false;
    const stops: (() => void)[] = [];
    const refresh = () => {
      if (reading) {
        again = true;
        return;
      }
      reading = true;
      mailStatus()
        .then((status) => {
          if (!disposed) setUnread(status.unreadTotal);
        })
        // A failed read keeps the last count rather than showing none.
        .catch(() => undefined)
        .finally(() => {
          reading = false;
          if (again && !disposed) {
            again = false;
            refresh();
          }
        });
    };
    refresh();
    for (const event of [MAIL_SYNC_STATE_EVENT, MAIL_MESSAGES_CHANGED_EVENT]) {
      void listen(event, refresh)
        .then((stop) => {
          if (disposed) stop();
          else stops.push(stop);
        })
        .catch(() => undefined);
    }
    return () => {
      disposed = true;
      for (const stop of stops) stop();
    };
  }, [enabled]);

  return unread;
}
