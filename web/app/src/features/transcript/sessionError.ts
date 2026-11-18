// Ported from mobile/src/features/sessions/sessionError.ts (F-41). The composer takeover bar and its
// dismiss-for-this-run store belong to Task 5; this file holds the pure parts the transcript needs.
import type { Turn } from "@monoptah/protocol";
import { readItems, type NoticeItem } from "./turnItems";

/** The latest error of the latest turn. Only that one can interrupt the reader; paged-in history never does. */
export function latestSessionError(turns: Turn[]): NoticeItem | undefined {
  const last = turns.at(-1);
  if (!last) return undefined;
  return readItems(last).findLast((item): item is NoticeItem => item.type === "notice" && item.error === true);
}

export interface SessionErrorContent {
  title: string;
  message: string;
  recovery: string | undefined;
}

/** Desktop errors sometimes refer to controls that only exist on the desktop. */
export function sessionErrorContent(text: string): SessionErrorContent {
  if (/\bremoved provider account\b/i.test(text)) {
    return {
      title: "Account unavailable",
      message: "The account used for this conversation has been removed.",
      recovery: "Open the model menu in the composer, choose Account, and pick another account to start a new conversation.",
    };
  }
  return { title: "Couldn't continue", message: text, recovery: undefined };
}
