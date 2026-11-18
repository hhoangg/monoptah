// What the turn components need from the screen that hosts them (the session screen here, the Mono chat
// of WS-P later). Everything is optional: a host that does not provide an action gets a read-only
// transcript (chips and cards disabled, file links shown unlinked).
import { createContext, use } from "react";
import type { DocumentItem, NoticeItem } from "./turnItems";

/** Who a Mono's turns are by, in place of the model and provider mark. */
export interface TranscriptAgent {
  name: string;
  /** `#rrggbb`; anything else is ignored. */
  color?: string;
}

export interface TranscriptActions {
  /** A notice row with `error` was pressed: the host shows the error dialog. */
  onError?: (notice: NoticeItem) => void;
  onOpenFile?: (path: string, line?: number) => void;
  onNavigate?: (href: string) => void;
  /** Sends a suggested reply as the user's message. Rejects when the send fails. */
  onChoose?: (text: string) => Promise<void>;
  onOpenDocument?: (document: DocumentItem) => void;
  /** Opens a session a Mono launched. */
  onOpenSession?: (projectId: string, sessionId: string) => void;
  /** False for a project outside this device's selection: the card shows "Not on this device's projects". */
  isProjectAvailable?: (projectId: string) => boolean;
}

export const TranscriptActionsContext = createContext<TranscriptActions>({});
export const useTranscriptActions = (): TranscriptActions => use(TranscriptActionsContext);

/**
 * True once the transcript has shown what was there when it opened. A reply that mounts after that is
 * new and fades in, as the desktop's revealOnMount.
 */
export const TranscriptOpened = createContext(false);
