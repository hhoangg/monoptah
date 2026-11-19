// Global keyboard shortcuts of the app shell (design-system.md section 11).
//
// Built in: Mod+K opens/closes the command palette, Esc closes the command palette, then the top-most
// right panel. Mod+P runs the handler registered for "quick-open", and does nothing (the browser keeps
// the key) while none is registered. Mod is Cmd on macOS and Ctrl elsewhere. The letter is matched by
// `event.key` and, for layouts that type another script, by the physical `event.code` (KeyK, KeyP).
// Mod+K / Mod+P never stack a second overlay: while a dialog or menu other than the palette is open they do
// nothing (the key is still taken, so the browser opens no address bar or print dialog), and Mod+P closes the
// palette before it runs quick-open. The right-panel sheet below 640px is not "another" overlay: Mod+K and
// Mod+P work over it. Nothing fires while an IME composes. A handled shortcut calls preventDefault.
//
// Public API:
//   type ShortcutId = "quick-open"
//   registerShortcut(id, handler): () => void   once per id at startup (a second call throws, like slots);
//                                               returns an unregister function
//   getShortcut(id): ShortcutHandler | undefined
//   clearShortcuts(): void                      for tests
//   isMod(event): boolean                       Cmd on macOS, Ctrl elsewhere
//   handleGlobalKeydown(event): boolean         the handler body (true when it handled the key)
//   useGlobalShortcuts(): void                  mounts the one window keydown listener (AppShell does)
//
// U-05 (Files) registers `registerShortcut("quick-open", () => ...)` from its register.ts.
import { useEffect } from "react";
import { closeCommandPalette, isCommandPaletteOpen, toggleCommandPalette } from "./palette";
import { closeTopmostPanel } from "./panels";

export type ShortcutId = "quick-open";
export type ShortcutHandler = () => void;

const handlers = new Map<ShortcutId, ShortcutHandler>();

export function registerShortcut(id: ShortcutId, handler: ShortcutHandler): () => void {
  if (handlers.has(id)) throw new Error(`Shortcut "${id}" is already registered`);
  handlers.set(id, handler);
  return () => {
    if (handlers.get(id) === handler) handlers.delete(id);
  };
}

export function getShortcut(id: ShortcutId): ShortcutHandler | undefined {
  return handlers.get(id);
}

/** Drops every registration. For tests. */
export function clearShortcuts(): void {
  handlers.clear();
}

/** True on Apple platforms, where Mod is Cmd. */
export function isMac(): boolean {
  return typeof navigator !== "undefined" && /Mac|iPhone|iPad|iPod/i.test(navigator.platform || navigator.userAgent);
}

/** Cmd on macOS, Ctrl elsewhere (and no other of the two). */
export function isMod(event: Pick<KeyboardEvent, "metaKey" | "ctrlKey">): boolean {
  return isMac() ? event.metaKey && !event.ctrlKey : event.ctrlKey && !event.metaKey;
}

const OVERLAY_SELECTOR = '[role="dialog"], [role="alertdialog"], [role="menu"], [role="listbox"]';

/** True while a dialog, sheet or menu is mounted anywhere, except the command palette's own (and its list). */
export function otherOverlayOpen(): boolean {
  const palette = isCommandPaletteOpen();
  for (const element of document.querySelectorAll(OVERLAY_SELECTOR)) {
    if (palette && (element.querySelector("[data-command-palette]") || element.closest("[data-command-palette]"))) continue;
    // A closed radix/vaul layer unmounts; a hidden one must not block the shortcut.
    if (element.closest("[hidden], [aria-hidden='true']")) continue;
    // The right-panel sheet (vaul, below 640px) is a dialog too, but it is the surface Mod+K and Mod+P work from.
    if (element.closest("[data-right-panel-sheet]")) continue;
    return true;
  }
  return false;
}

/** The pressed letter: `event.key` when it is Latin, else the physical key (non-Latin layouts). */
function pressedLetter(event: KeyboardEvent): string {
  const key = event.key.toLowerCase();
  if (/^[a-z]$/.test(key)) return key;
  return /^Key[A-Z]$/.test(event.code) ? event.code.slice(3).toLowerCase() : key;
}

/** An open dialog, sheet or menu other than the panel host owns Esc. */
function insideOtherOverlay(target: EventTarget | null): boolean {
  if (!(target instanceof Element)) return false;
  return target.closest('[role="dialog"], [role="alertdialog"], [role="menu"], [role="listbox"]') !== null;
}

export function handleGlobalKeydown(event: KeyboardEvent): boolean {
  if (event.defaultPrevented || event.isComposing || event.keyCode === 229) return false;
  if (event.key === "Escape") {
    if (isCommandPaletteOpen()) {
      closeCommandPalette();
      event.preventDefault();
      return true;
    }
    if (insideOtherOverlay(event.target)) return false;
    if (closeTopmostPanel()) {
      event.preventDefault();
      return true;
    }
    return false;
  }
  if (!isMod(event) || event.altKey || event.shiftKey) return false;
  const key = pressedLetter(event);
  if (key === "k") {
    // The key is the app's: the browser must not get it (address bar), even while an overlay blocks it.
    event.preventDefault();
    if (otherOverlayOpen()) return true;
    toggleCommandPalette();
    return true;
  }
  if (key === "p") {
    const handler = handlers.get("quick-open");
    if (!handler) return false;
    // Same for Mod+P (print dialog): swallowed and ignored while another overlay is open.
    event.preventDefault();
    if (otherOverlayOpen()) return true;
    closeCommandPalette();
    handler();
    return true;
  }
  return false;
}

/** Mounts the single global keydown listener. */
export function useGlobalShortcuts(): void {
  useEffect(() => {
    const listener = (event: KeyboardEvent) => void handleGlobalKeydown(event);
    window.addEventListener("keydown", listener);
    return () => window.removeEventListener("keydown", listener);
  }, []);
}
