import { useCallback, useEffect, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import {
  MAIL_MESSAGES_CHANGED_EVENT,
  MAIL_SYNC_STATE_EVENT,
  mailStatus,
} from "./mail";
import type { MailStatus } from "./mailTypes";

/** Status for the whole Mail view, refreshed when sync state or messages change. */
export function useMailStatus() {
  const [status, setStatus] = useState<MailStatus | null>(null);
  const [error, setError] = useState<string | null>(null);
  const refresh = useCallback(async () => {
    try {
      setStatus(await mailStatus());
      setError(null);
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    }
  }, []);

  useEffect(() => {
    let disposed = false;
    const stops: (() => void)[] = [];
    void refresh();
    for (const event of [MAIL_SYNC_STATE_EVENT, MAIL_MESSAGES_CHANGED_EVENT]) {
      void listen(event, () => void refresh())
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
  }, [refresh]);

  return { status, error, refresh };
}
