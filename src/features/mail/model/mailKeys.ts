/**
 * The Mail view's keyboard map, as a pure decision from one keydown. The view
 * installs the listener only while it is mounted, so nothing here has to know
 * which view is showing; it only has to know when a key is not Mail's to take.
 */

export type MailKeyAction =
  | "next"
  | "prev"
  | "open"
  | "reply"
  | "reply_all"
  | "archive"
  | "delete"
  | "search"
  | "back";

const KEYS: Record<string, MailKeyAction> = {
  j: "next",
  k: "prev",
  Enter: "open",
  r: "reply",
  a: "reply_all",
  e: "archive",
  "#": "delete",
  "/": "search",
  Escape: "back",
};

/** Text entry, and surfaces that own the keyboard while they are open. */
const OWNS_KEYS =
  'input, textarea, select, [contenteditable]:not([contenteditable="false"]), .cm-editor, [role="dialog"], iframe';

/** What Enter already does when it is focused, so it must not be taken. */
const ACTIVATES =
  'button, a[href], summary, [role="button"], [role="switch"], [role="radio"], [role="tab"]';

type KeyLike = Pick<
  KeyboardEvent,
  | "key"
  | "ctrlKey"
  | "metaKey"
  | "altKey"
  | "repeat"
  | "isComposing"
  | "defaultPrevented"
> & { target: EventTarget | null };

export function mailKeyAction(event: KeyLike): MailKeyAction | null {
  if (event.defaultPrevented || event.isComposing) return null;
  // Shift is allowed because `#` needs it; Cmd/Ctrl/Alt belong to the app.
  if (event.ctrlKey || event.metaKey || event.altKey) return null;
  const action = KEYS[event.key];
  if (!action) return null;
  // Held down, only moving repeats; a held `e` would archive the whole list.
  if (event.repeat && action !== "next" && action !== "prev") return null;
  const target = event.target instanceof Element ? event.target : null;
  if (target?.closest(OWNS_KEYS)) return null;
  if (action === "open" && target?.closest(ACTIVATES)) return null;
  return action;
}
