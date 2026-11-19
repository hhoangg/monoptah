// Keyboard shortcuts of the tabs and panes. Mod is Cmd on macOS and Ctrl elsewhere (isMod). The native app binds
// close tab to Mod+W, next tab to Mod+Shift+], tab n to Mod+n and focus pane to Mod+Alt+arrows, but a web page cannot
// take Mod+W, Mod+T, Ctrl+Tab, Mod+1..9 or Mod+Alt+arrows from the browser, so those use Mod+Alt instead:
//
//   Mod+D                  split right (native: the same)
//   Mod+Shift+D            split down (native: the same)
//   Mod+Alt+W              close the tab (native: Mod+W)
//   Mod+Alt+]  /  Mod+Alt+[   next / previous tab (native: Mod+Shift+] / Mod+Shift+[)
//   Mod+Alt+1 .. 9         tab 1..8, 9 is the last tab (native: Mod+1..9)
//   Mod+Alt+Shift+arrows   focus the next or previous pane (native: Mod+Alt+arrows)
//
// Keys are matched by `event.code`, because Option on a Mac types another character.
//
// AltGr: Windows reports AltGr as Ctrl+Alt, so a Mod+Alt chord could swallow the characters an AltGr layout types
// ({ [ ] | \ # ^ and so on). Off the Mac a Mod+Alt event that produced a printable character other than the plain
// character of its key (Ctrl+Alt+Digit8 giving "[") is AltGr typing, not a shortcut, and the Mod+Alt chords are left
// alone while the focus is in a text field. The AltGraph modifier itself is not used: Firefox reports it for Option on a
// Mac and for every Ctrl+Alt on Windows, so it cannot tell a shortcut from typing.
import { useEffect } from "react";
import { isMac, isMod, otherOverlayOpen } from "../shortcuts";
import type { TabActions } from "./useTabActions";

/** True when the event comes from somewhere text is typed: an input, a textarea, a select or contenteditable content. */
function inEditable(event: KeyboardEvent): boolean {
  const target = event.target;
  if (!(target instanceof HTMLElement)) return false;
  return target.isContentEditable || target.closest("input, textarea, select, [contenteditable]:not([contenteditable=false])") !== null;
}

/** The character a key types with no modifier on a US layout, for the keys the Mod+Alt shortcuts use. */
function plainCharacter(code: string): string | null {
  const letter = /^Key([A-Z])$/.exec(code)?.[1];
  if (letter) return letter.toLowerCase();
  const digit = /^Digit([0-9])$/.exec(code)?.[1];
  if (digit) return digit;
  if (code === "BracketLeft") return "[";
  if (code === "BracketRight") return "]";
  return null;
}

/** True for a Ctrl+Alt event that typed a character the key does not type on its own: AltGr typing. */
function typedByAltGr(event: KeyboardEvent): boolean {
  if (event.key.length !== 1 && [...event.key].length !== 1) return false;
  const plain = plainCharacter(event.code);
  return plain === null || event.key.toLowerCase() !== plain;
}

/** Runs the tab shortcut of `event`; true when it was one (the browser's own action is cancelled). */
export function handleTabKeydown(event: KeyboardEvent, actions: TabActions): boolean {
  if (event.defaultPrevented || event.isComposing || event.keyCode === 229 || !isMod(event)) return false;
  if (event.altKey && !isMac()) {
    // Ctrl+Alt is also how AltGr layouts type characters: those events are typing, and a focused field keeps the chords.
    if (typedByAltGr(event) || inEditable(event)) return false;
  }
  const run = (action: () => void) => {
    event.preventDefault();
    // Under a dialog or menu the key is still taken (the browser must not bookmark or close), but nothing runs.
    if (!otherOverlayOpen()) action();
    return true;
  };
  if (!event.altKey) {
    if (event.code === "KeyD") return run(() => actions.split(event.shiftKey ? "column" : "row"));
    return false;
  }
  if (event.shiftKey) {
    if (event.code === "ArrowRight" || event.code === "ArrowDown") return run(() => actions.focusAdjacent(1));
    if (event.code === "ArrowLeft" || event.code === "ArrowUp") return run(() => actions.focusAdjacent(-1));
    return false;
  }
  if (event.code === "KeyW") return run(() => actions.closeActive());
  if (event.code === "BracketRight") return run(() => actions.cycle(1));
  if (event.code === "BracketLeft") return run(() => actions.cycle(-1));
  const digit = /^Digit([1-9])$/.exec(event.code)?.[1];
  if (digit) return run(() => actions.activateIndex(digit === "9" ? -1 : Number(digit) - 1));
  return false;
}

export function useTabShortcuts(actions: TabActions): void {
  useEffect(() => {
    const listener = (event: KeyboardEvent) => void handleTabKeydown(event, actions);
    window.addEventListener("keydown", listener);
    return () => window.removeEventListener("keydown", listener);
  }, [actions]);
}
