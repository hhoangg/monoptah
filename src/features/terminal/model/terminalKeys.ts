type TerminalKeyEvent = Pick<
  KeyboardEvent,
  "key" | "altKey" | "ctrlKey" | "metaKey" | "shiftKey"
>;

/** Translate macOS editing shortcuts into sequences understood by common shells. */
export function macTerminalShortcutData(
  event: TerminalKeyEvent,
): string | null {
  if (event.ctrlKey || event.shiftKey) return null;

  if (event.altKey && !event.metaKey) {
    if (event.key === "ArrowLeft") return "\x1bb";
    if (event.key === "ArrowRight") return "\x1bf";
    return null;
  }

  if (event.metaKey && !event.altKey) {
    if (event.key === "ArrowLeft") return "\x01";
    if (event.key === "ArrowRight") return "\x05";
    if (event.key === "Backspace") return "\x15";
  }

  return null;
}

/**
 * Whether a key press is the macOS Cmd+K clear. It only reaches the terminal
 * when "App: Search" is disabled or rebound, since the app claims it first.
 */
export function isMacTerminalClearShortcut(event: TerminalKeyEvent): boolean {
  return (
    event.metaKey &&
    !event.ctrlKey &&
    !event.altKey &&
    !event.shiftKey &&
    event.key.toLowerCase() === "k"
  );
}

/**
 * Named keys from the UI Events spec ("Enter", "ArrowLeft", "F11", "Dead",
 * "MediaPlayPause", ...) are alphanumeric and start with an uppercase letter.
 */
const NAMED_KEY_SHAPE = /^[A-Z][A-Za-z0-9]*$/;
const CONTROL_CHARACTER = /\p{Cc}/u;

/**
 * The text of a keydown whose `key` is a whole string rather than one
 * character, or null when xterm should handle the event itself.
 *
 * Some input methods (GoTiengViet on macOS) erase the typed letters and then
 * deliver the replacement as a single non-composing keydown, e.g. `key` is
 * "ếng". xterm emits only the first character of that, so the rest is lost.
 *
 * Multi-character ASCII is text unless it looks like a named key. A lowercase
 * start ("eng", after the method strips a tone mark) cannot be a spec key
 * name, so it is text. A capitalised alphanumeric word ("Eng", "Enter") is
 * treated as a key name: sending an unknown name such as "MediaPlayPause" to
 * the shell would be worse than dropping the rare capitalised ASCII string.
 */
export function multiCharacterKeyText(
  event: TerminalKeyEvent & Partial<Pick<KeyboardEvent, "isComposing">>,
): string | null {
  // Shortcut paths belong to the handlers that already own them, and a live
  // composition is xterm's CompositionHelper's job.
  if (event.ctrlKey || event.metaKey || event.altKey || event.isComposing) {
    return null;
  }
  const { key } = event;
  // Counted by code point so one astral character stays a single character.
  if ([...key].length < 2) return null;
  if (NAMED_KEY_SHAPE.test(key) || CONTROL_CHARACTER.test(key)) return null;
  return key;
}
