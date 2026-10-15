import {
  useEffect,
  useRef,
  useState,
  type KeyboardEvent,
  type ReactNode,
} from "react";
import { Popover } from "../../../../shared/ui/Popover";
import type { PersonRow } from "../../model/unmapped";
import { Avatar } from "../Avatar";

const ITEM =
  "flex w-full items-center gap-2 rounded-md px-2.5 py-1.5 text-left text-[12px] text-content/80 hover:bg-content/8 hover:text-content focus-visible:bg-content/8 focus-visible:outline-none disabled:cursor-default disabled:opacity-40";

const ENABLED_ITEM = '[role="menuitem"]:not(:disabled)';

/** Moves focus between the items of an open menu with the arrow, Home and End keys. */
function onMenuKey(event: KeyboardEvent<HTMLDivElement>) {
  const keys = ["ArrowDown", "ArrowUp", "Home", "End"];
  if (!keys.includes(event.key)) return;
  const items = Array.from(
    event.currentTarget.querySelectorAll<HTMLElement>(ENABLED_ITEM),
  );
  if (items.length === 0) return;
  event.preventDefault();
  const at = items.indexOf(document.activeElement as HTMLElement);
  let next: number;
  if (event.key === "Home") next = 0;
  else if (event.key === "End") next = items.length - 1;
  else if (event.key === "ArrowDown")
    next = at < 0 ? 0 : (at + 1) % items.length;
  else
    next = at < 0 ? items.length - 1 : (at - 1 + items.length) % items.length;
  items[next].focus();
}

/**
 * Focuses the first enabled item of the menu it renders in (or the menu itself
 * when it has none). Mount it again with a new key when the content changes.
 * The popover is hidden until it is placed, so a refused focus is retried once
 * the browser has laid it out.
 */
export function FocusFirstItem() {
  const marker = useRef<HTMLSpanElement>(null);
  useEffect(() => {
    const menu = marker.current?.closest<HTMLElement>('[role="menu"]');
    if (!menu) return;
    const focus = () => {
      const target = menu.querySelector<HTMLElement>(ENABLED_ITEM) ?? menu;
      target.focus();
      return document.activeElement === target;
    };
    if (focus()) return;
    const frame = requestAnimationFrame(focus);
    return () => cancelAnimationFrame(frame);
  }, []);
  return <span ref={marker} hidden />;
}

export function MenuItem({
  children,
  description,
  onSelect,
  disabled,
  danger,
}: {
  children: ReactNode;
  /** A short sentence under the label saying what the item will do. */
  description?: string;
  onSelect: () => void;
  disabled?: boolean;
  danger?: boolean;
}) {
  return (
    <button
      type="button"
      role="menuitem"
      disabled={disabled}
      onClick={onSelect}
      className={`${ITEM} ${danger ? "text-red-400 hover:text-red-400" : ""}`}
    >
      {description ? (
        <span className="flex min-w-0 flex-col">
          <span>{children}</span>
          <span className="text-[11px] font-normal text-content/50">
            {description}
          </span>
        </span>
      ) : (
        children
      )}
    </button>
  );
}

/** A themed menu popover; Escape and outside clicks close it and focus returns to the trigger. */
export function Menu({
  anchor,
  label,
  onClose,
  children,
}: {
  anchor: HTMLElement | null;
  label: string;
  onClose: () => void;
  children: ReactNode;
}) {
  return (
    <Popover
      anchor={anchor}
      side="bottom"
      align="end"
      width={240}
      maxHeight={320}
      onDismiss={(reason) => {
        onClose();
        if (reason === "escape") anchor?.focus();
      }}
      role="menu"
      aria-label={label}
      tabIndex={-1}
      onKeyDown={onMenuKey}
      className="overflow-y-auto overscroll-contain p-1"
    >
      <FocusFirstItem />
      {children}
    </Popover>
  );
}

/** A button that opens a menu built by `children(close)`. */
export function MenuButton({
  label,
  trigger,
  triggerClassName,
  menuLabel,
  disabled = false,
  children,
}: {
  label: string;
  trigger: ReactNode;
  triggerClassName: string;
  menuLabel: string;
  disabled?: boolean;
  children: (close: () => void) => ReactNode;
}) {
  const [open, setOpen] = useState(false);
  const button = useRef<HTMLButtonElement>(null);
  const close = () => setOpen(false);
  return (
    <>
      <button
        ref={button}
        type="button"
        aria-label={label}
        aria-haspopup="menu"
        aria-expanded={open}
        disabled={disabled}
        onClick={() => setOpen((v) => !v)}
        className={`${triggerClassName}${disabled ? " opacity-40" : ""}`}
      >
        {trigger}
      </button>
      {open ? (
        <Menu anchor={button.current} label={menuLabel} onClose={close}>
          {children(close)}
        </Menu>
      ) : null}
    </>
  );
}

/** Menu entries that list people; the caller decides what picking one does. */
export function PeopleItems({
  people,
  colorOf,
  onPick,
  empty = "No other people",
}: {
  people: PersonRow[];
  colorOf: (id: string) => string;
  onPick: (person: PersonRow) => void;
  empty?: string;
}) {
  if (people.length === 0) {
    return (
      <div className="px-2.5 py-1.5 text-[12px] text-content/45">{empty}</div>
    );
  }
  return (
    <>
      {people.map((p) => (
        <MenuItem key={p.id} onSelect={() => onPick(p)}>
          <Avatar name={p.name} color={colorOf(p.id)} size="sm" />
          <span className="min-w-0 flex-1 truncate">{p.name}</span>
        </MenuItem>
      ))}
    </>
  );
}
