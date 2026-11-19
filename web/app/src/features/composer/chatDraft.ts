// What a person has typed in the composer of an existing session or a Mono chat, kept while the chat is not on screen
// (a tab switch, a split or a closed pane remounts the screen). In memory only, for the life of the page, like
// `features/sessions/newSessionDraft.ts`: nothing is written to storage, and attachments (files) are not kept.
// The composer reports every change; a sent message empties the field, which drops the draft.

const drafts = new Map<string, string>();
const keyOf = (desktopId: string, chatId: string) => `${desktopId}:${chatId}`;

/** The draft of a chat (`chatId` is the session or Mono conversation id), or "". */
export function readChatDraft(desktopId: string, chatId: string): string {
  return drafts.get(keyOf(desktopId, chatId)) ?? "";
}

export function writeChatDraft(desktopId: string, chatId: string, text: string): void {
  if (text === "") drafts.delete(keyOf(desktopId, chatId));
  else drafts.set(keyOf(desktopId, chatId), text);
}

export function resetChatDrafts(): void {
  drafts.clear();
}

/** Drops every draft of one desktop (it was forgotten on this device or its link was revoked). */
export function forgetChatDrafts(desktopId: string): void {
  for (const key of [...drafts.keys()]) if (key.startsWith(`${desktopId}:`)) drafts.delete(key);
}
