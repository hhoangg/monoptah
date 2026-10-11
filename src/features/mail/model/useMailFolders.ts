import { useCallback, useEffect, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import { MAIL_MESSAGES_CHANGED_EVENT, mailFolders } from "./mail";
import type { FolderRow } from "./mailTypes";

/**
 * The Archive view is selected through the folder pref, as this reserved word.
 * It is not a folder id, so `parseFolderId` rejects it and nothing is stored in
 * the folder table for it.
 */
export const ARCHIVE_PREF = "archived";

/**
 * The selected folder is persisted as text next to the mailbox pref. This is
 * the one place text becomes an id, and anything that is not a plain positive
 * integer is no folder at all, so a damaged pref can never become `NaN`.
 */
export function parseFolderId(pref: string | null): number | null {
  if (pref === null || !/^[1-9]\d*$/.test(pref)) return null;
  const id = Number(pref);
  return Number.isSafeInteger(id) ? id : null;
}

/**
 * Smart folders, refreshed when mail changes: a message that arrives later can
 * be filed into a folder, which moves its counts and its listing.
 */
export function useMailFolders() {
  const [folders, setFolders] = useState<FolderRow[] | null>(null);
  const [error, setError] = useState<string | null>(null);
  const refresh = useCallback(async () => {
    try {
      setFolders(await mailFolders());
      setError(null);
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    }
  }, []);

  useEffect(() => {
    let disposed = false;
    let stop: (() => void) | undefined;
    void refresh();
    void listen(MAIL_MESSAGES_CHANGED_EVENT, () => void refresh())
      .then((unlisten) => {
        if (disposed) unlisten();
        else stop = unlisten;
      })
      .catch(() => undefined);
    return () => {
      disposed = true;
      stop?.();
    };
  }, [refresh]);

  return { folders, error, refresh };
}
