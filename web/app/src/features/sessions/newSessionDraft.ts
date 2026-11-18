// What a person has typed or picked on the New session page, kept per project while the page is not on screen
// (F-18: the wide side pane unmounts when the window narrows, and comes back when it widens). In memory only, for
// the life of the tab: no secrets are stored, and attachments (files) are not kept.
import { useEffect } from "react";
import type { ModelSelection } from "@/features/models/modelSettings";
import { getLinkStore } from "@/link";

export interface NewSessionDraft {
  text: string;
  picked: ModelSelection | null;
}

const drafts = new Map<string, NewSessionDraft>();
const keyOf = (desktopId: string, projectId: string) => `${desktopId}:${projectId}`;

const EMPTY: NewSessionDraft = { text: "", picked: null };

export function readNewSessionDraft(desktopId: string, projectId: string): NewSessionDraft {
  return drafts.get(keyOf(desktopId, projectId)) ?? EMPTY;
}

export function writeNewSessionDraft(desktopId: string, projectId: string, patch: Partial<NewSessionDraft>): void {
  const next = { ...readNewSessionDraft(desktopId, projectId), ...patch };
  if (next.text === "" && next.picked === null) drafts.delete(keyOf(desktopId, projectId));
  else drafts.set(keyOf(desktopId, projectId), next);
}

export function clearNewSessionDraft(desktopId: string, projectId: string): void {
  drafts.delete(keyOf(desktopId, projectId));
}

export function resetNewSessionDrafts(): void {
  drafts.clear();
}

/** Drops every draft of one desktop (it was forgotten on this device or its link was revoked). */
export function forgetNewSessionDrafts(desktopId: string): void {
  for (const key of [...drafts.keys()]) if (key.startsWith(`${desktopId}:`)) drafts.delete(key);
}

/** A revoked link takes the desktop's drafts with it (read-only look at the link store). */
export function useForgetDraftsWhenRevoked(desktopId: string): void {
  useEffect(() => {
    const store = getLinkStore(desktopId);
    const check = (link: { revoked: boolean; blocked: string | null }) => {
      if (link.revoked || link.blocked === "revoked") forgetNewSessionDrafts(desktopId);
    };
    check(store.getState());
    return store.subscribe(check);
  }, [desktopId]);
}
