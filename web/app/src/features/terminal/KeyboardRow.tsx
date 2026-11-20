// The keys a phone keyboard lacks: Esc, Tab, sticky Ctrl and Alt, the arrows and a few symbols. Everything goes out
// through `onSend`, which is the same write path as the typed keys, so the sticky modifiers apply to both.
import type { ReactNode } from "react";
import { cn } from "@/lib/utils";
import { KEY_ESC, KEY_TAB, SYMBOL_KEYS, arrowSequence, type Arrow, type Modifiers } from "./input";

export interface KeyboardRowProps {
  modifiers: Modifiers;
  onModifiers: (next: Modifiers) => void;
  /** The text of a key; the caller applies the modifiers and clears them. */
  onSend: (text: string) => void;
  applicationCursor: () => boolean;
}

const KEY =
  "inline-flex min-h-11 min-w-11 shrink-0 items-center justify-center rounded-row border border-stroke bg-fill px-3 text-label font-medium text-content select-none hover:bg-selection aria-pressed:bg-content aria-pressed:text-background";

/** A press must not move focus off the terminal: the soft keyboard would close and reopen. */
const keepFocus = (event: { preventDefault: () => void }): void => event.preventDefault();

const ARROWS: ReadonlyArray<{ arrow: Arrow; label: string; glyph: string }> = [
  { arrow: "left", label: "Left arrow", glyph: "←" },
  { arrow: "up", label: "Up arrow", glyph: "↑" },
  { arrow: "down", label: "Down arrow", glyph: "↓" },
  { arrow: "right", label: "Right arrow", glyph: "→" },
];

export function KeyboardRow({ modifiers, onModifiers, onSend, applicationCursor }: KeyboardRowProps) {
  const key = (label: string, text: string, content?: ReactNode, name = label) => (
    <button key={name} type="button" aria-label={name} data-key={name} className={KEY} onMouseDown={keepFocus} onClick={() => onSend(text)}>
      {content ?? label}
    </button>
  );
  const sticky = (name: "Ctrl" | "Alt", field: keyof Modifiers) => (
    <button
      type="button"
      aria-label={name}
      aria-pressed={modifiers[field]}
      data-key={name}
      className={KEY}
      onMouseDown={keepFocus}
      onClick={() => onModifiers({ ...modifiers, [field]: !modifiers[field] })}
    >
      {name}
    </button>
  );
  return (
    <div role="toolbar" aria-label="Terminal keys" data-testid="terminal-keys" className={cn("flex gap-1.5 overflow-x-auto overscroll-contain border-t border-stroke bg-sheet px-2 py-1.5 [scrollbar-width:none]")}>
      {key("Esc", KEY_ESC)}
      {key("Tab", KEY_TAB)}
      {sticky("Ctrl", "ctrl")}
      {sticky("Alt", "alt")}
      {ARROWS.map(({ arrow, label, glyph }) => (
        <button key={arrow} type="button" aria-label={label} data-key={label} className={KEY} onMouseDown={keepFocus} onClick={() => onSend(arrowSequence(arrow, applicationCursor()))}>
          <span aria-hidden="true">{glyph}</span>
        </button>
      ))}
      {SYMBOL_KEYS.map((symbol) => key(symbol, symbol, undefined, `Symbol ${symbol}`))}
    </div>
  );
}
