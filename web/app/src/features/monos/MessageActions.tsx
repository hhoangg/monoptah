// F-71: message actions in a Mono's chat. A context menu (right click, or a long press on touch) on a message offers
// 12 emoji reactions (Mono messages only; stored in IndexedDB on this device, see reactions.ts), Reply (control
// devices only), Mark unread (Mono messages) and Copy. Selecting text, and a right click on a link, keep the
// browser's own menu. The menu also opens from the keyboard (Shift+F10, the Menu key, or the "Message actions"
// button each message shows on focus). The transcript is a WS-T component with no per-message hook, so the
// messages are found by the `data-chat-message-role` and `data-turn-id` attributes it renders; a reaction and the
// button are drawn into their message through a React portal.
import { useEffect, useRef, useState, type ReactNode } from "react";
import { createPortal } from "react-dom";
import { toast } from "@/ui/sonner";
import { Icon } from "@/ui/Icon";
import { ContextMenu, ContextMenuContent, ContextMenuItem, ContextMenuSeparator, ContextMenuTrigger } from "./contextMenu";
import { REACTION_EMOJIS, messageKey, setReaction, useMonoReactions } from "./reactions";

export interface HeldMessage {
  key: string;
  role: "user" | "assistant";
  text: string;
}

const MESSAGE = "[data-chat-message-role]";

/** The message under an event target, with its key; null outside a message. */
export function messageAt(target: EventTarget | null): HeldMessage | null {
  const element = target instanceof Element ? target.closest<HTMLElement>(MESSAGE) : null;
  const turn = element?.closest<HTMLElement>("[data-turn-id]");
  const turnId = turn?.dataset.turnId;
  if (!element || !turn || !turnId) return null;
  const index = [...turn.querySelectorAll(MESSAGE)].indexOf(element);
  const role = element.dataset.chatMessageRole === "user" ? "user" : "assistant";
  // The reactions drawn into the message are not part of what it says.
  const copy = element.cloneNode(true) as HTMLElement;
  copy.querySelectorAll("[data-message-extras]").forEach((holder) => holder.remove());
  return { key: messageKey(turnId, Math.max(index, 0)), role, text: (copy.textContent ?? "").trim() };
}

/** True when the user selected text inside the message under `target`, or aims at a link: the browser's menu stays. */
function wantsNativeMenu(target: EventTarget | null): boolean {
  if (!(target instanceof Element)) return false;
  // The "Message actions" button asks for the menu itself.
  if (target.closest("[data-message-extras]")) return false;
  if (target.closest("a[href]")) return true;
  const message = target.closest<HTMLElement>(MESSAGE);
  const selection = typeof window.getSelection === "function" ? window.getSelection() : null;
  return !!message && !!selection && !selection.isCollapsed && !!selection.anchorNode && message.contains(selection.anchorNode);
}

/** Opens the context menu at the middle of `element`, as a right click there would. */
function openMenuAt(element: Element): void {
  const box = element.getBoundingClientRect();
  element.dispatchEvent(new MouseEvent("contextmenu", { bubbles: true, cancelable: true, clientX: box.left + box.width / 2, clientY: box.top + box.height / 2 }));
}

export function MessageActions({
  desktopId,
  monoId,
  readOnly,
  onReply,
  onMarkUnread,
  className,
  children,
}: {
  desktopId: string;
  monoId: string;
  readOnly: boolean;
  onReply: (message: HeldMessage) => void;
  onMarkUnread: () => void;
  className?: string;
  children: ReactNode;
}) {
  const root = useRef<HTMLDivElement>(null);
  const [held, setHeldState] = useState<HeldMessage | null>(null);
  // The check in onOpenChange runs in the same event as the capture handler that sets the message, before a render.
  const heldRef = useRef<HeldMessage | null>(null);
  const [open, setOpen] = useState(false);
  const reactions = useMonoReactions(desktopId, monoId);

  const setHeld = (message: HeldMessage | null) => {
    heldRef.current = message;
    setHeldState(message);
  };

  const copy = (message: HeldMessage) => {
    // The click of the menu item is the user gesture the clipboard needs.
    const write = globalThis.navigator?.clipboard?.writeText(message.text);
    if (!write) {
      toast.error("Could not copy the message");
      return;
    }
    write.then(
      () => toast.success("Copied"),
      () => toast.error("Could not copy the message"),
    );
  };

  return (
    <ContextMenu
      open={open}
      onOpenChange={(next) => {
        // Nothing to act on outside a message (or on a link or a selection): the menu stays shut.
        if (next && !heldRef.current) return;
        setOpen(next);
      }}
    >
      <ContextMenuTrigger asChild>
        <div
          ref={root}
          className={className}
          // iOS fires no `contextmenu` on a long press: Radix opens the menu from a pointerdown timer, so the
          // message under the finger is taken here too.
          onPointerDownCapture={(event) => setHeld(wantsNativeMenu(event.target) ? null : messageAt(event.target))}
          onContextMenuCapture={(event) => {
            const message = wantsNativeMenu(event.target) ? null : messageAt(event.target);
            setHeld(message);
            // Outside a message, or on a link or a selection, the browser's own menu stays.
            if (!message) event.stopPropagation();
          }}
          onKeyDown={(event) => {
            if (!(event.key === "ContextMenu" || (event.key === "F10" && event.shiftKey))) return;
            if (!(event.target instanceof Element) || !event.target.closest(MESSAGE)) return;
            event.preventDefault();
            openMenuAt(event.target);
          }}
        >
          {children}
          <MessageExtras root={root} reactions={reactions} />
        </div>
      </ContextMenuTrigger>
      <ContextMenuContent>
        {held?.role === "assistant" && !readOnly ? (
          <>
            <div role="group" aria-label="Reactions" className="grid grid-cols-6 gap-0.5 p-1">
              {REACTION_EMOJIS.map((emoji) => (
                <ContextMenuItem
                  key={emoji}
                  aria-label={`React ${emoji}`}
                  data-selected={held && reactions[held.key] === emoji ? "true" : undefined}
                  className="min-h-9 justify-center gap-0 px-0 text-[20px] data-[selected=true]:bg-selection pointer-fine:min-h-9"
                  onSelect={() => held && setReaction(desktopId, monoId, held.key, emoji)}
                >
                  {emoji}
                </ContextMenuItem>
              ))}
            </div>
            <ContextMenuSeparator />
          </>
        ) : null}
        {readOnly ? null : (
          <ContextMenuItem onSelect={() => held && onReply(held)}>
            <Icon name="chat" size={18} />
            Reply
          </ContextMenuItem>
        )}
        {held?.role === "assistant" ? (
          <ContextMenuItem onSelect={onMarkUnread}>
            <Icon name="inbox" size={18} />
            Mark unread
          </ContextMenuItem>
        ) : null}
        <ContextMenuItem onSelect={() => held && copy(held)}>
          <Icon name="copy" size={18} />
          Copy
        </ContextMenuItem>
      </ContextMenuContent>
    </ContextMenu>
  );
}

/**
 * Draws into every message, wherever the (virtualised) transcript has it mounted: its stored reaction under it and
 * a "Message actions" button that shows on keyboard focus, the keyboard path to the menu (F-71).
 */
function MessageExtras({ root, reactions }: { root: React.RefObject<HTMLDivElement | null>; reactions: Readonly<Record<string, string>> }) {
  const [targets, setTargets] = useState<ReadonlyMap<string, HTMLElement>>(new Map());
  const frame = useRef(0);
  const holders = useRef(new Map<string, HTMLElement>());

  useEffect(() => {
    const element = root.current;
    if (!element) return;
    const scan = () => {
      const next = new Map<string, HTMLElement>();
      for (const turn of element.querySelectorAll<HTMLElement>("[data-turn-id]")) {
        const turnId = turn.dataset.turnId;
        if (!turnId) continue;
        turn.querySelectorAll<HTMLElement>(MESSAGE).forEach((message, index) => {
          const key = messageKey(turnId, index);
          let holder = holders.current.get(key);
          if (!holder || holder.parentElement !== message) {
            holder?.remove();
            holder = document.createElement("div");
            holder.dataset.messageExtras = key;
            message.append(holder);
            holders.current.set(key, holder);
          }
          next.set(key, holder);
        });
      }
      for (const [key, holder] of holders.current) {
        if (!next.has(key)) {
          holder.remove();
          holders.current.delete(key);
        }
      }
      setTargets((previous) => (previous.size === next.size && [...next].every(([key, holder]) => previous.get(key) === holder) ? previous : next));
    };
    const schedule = () => {
      cancelAnimationFrame(frame.current);
      frame.current = requestAnimationFrame(scan);
    };
    schedule();
    if (typeof MutationObserver === "undefined") return () => cancelAnimationFrame(frame.current);
    // The transcript mounts and unmounts messages as it scrolls: look again when its content changes.
    const observer = new MutationObserver(schedule);
    observer.observe(element, { childList: true, subtree: true });
    return () => {
      observer.disconnect();
      cancelAnimationFrame(frame.current);
    };
  }, [root]);

  useEffect(() => {
    const held = holders.current;
    return () => {
      held.forEach((holder) => holder.remove());
      held.clear();
    };
  }, []);

  return (
    <>
      {[...targets].map(([key, holder]) => {
        const emoji = reactions[key];
        return createPortal(
          <>
            {emoji ? (
              <span data-testid="message-reaction" className="mt-1 ml-5 inline-flex items-center rounded-pill bg-[var(--content-a07)] px-2 py-0.5 text-small" role="img" aria-label={`Reacted ${emoji}`}>
                {emoji}
              </span>
            ) : null}
            <button
              type="button"
              aria-label="Message actions"
              className="mt-1 ml-5 sr-only min-h-11 items-center gap-1.5 rounded-pill bg-fill px-3 text-small text-muted-text focus-visible:not-sr-only focus-visible:inline-flex pointer-fine:min-h-9"
              onClick={(event) => openMenuAt(event.currentTarget)}
            >
              <Icon name="more" size={16} />
              More
            </button>
          </>,
          holder,
          key,
        );
      })}
    </>
  );
}
