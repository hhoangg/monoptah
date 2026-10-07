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
 * Named `key` values from the UI Events spec (plus the legacy ones browsers
 * still send). The list is finite, so anything multi-character outside it is
 * text, including capitalised ASCII such as "Tien" that an input method
 * resends after a tone mark is removed.
 */
const NAMED_KEYS: ReadonlySet<string> = new Set([
  // Special and modifier keys.
  "Unidentified", "Alt", "AltGraph", "CapsLock", "Control", "Fn", "FnLock",
  "Meta", "NumLock", "ScrollLock", "Shift", "Symbol", "SymbolLock", "Hyper",
  "Super", "OS", "Win",
  // Whitespace and navigation.
  "Enter", "Tab", "ArrowDown", "ArrowLeft", "ArrowRight", "ArrowUp", "End",
  "Home", "PageDown", "PageUp", "Down", "Left", "Right", "Up",
  // Editing.
  "Backspace", "Clear", "Copy", "CrSel", "Cut", "Delete", "EraseEof",
  "ExSel", "Insert", "Paste", "Redo", "Undo",
  // UI.
  "Accept", "Again", "Attn", "Cancel", "ContextMenu", "Escape", "Execute",
  "Find", "Finish", "Help", "Pause", "Play", "Props", "Select", "ZoomIn",
  "ZoomOut", "Apps", "Esc", "Scroll", "Spacebar", "Del",
  // Device.
  "BrightnessDown", "BrightnessUp", "Eject", "LogOff", "Power",
  "PowerOff", "PrintScreen", "Hibernate", "Standby", "WakeUp",
  // IME and composition.
  "AllCandidates", "Alphanumeric", "CodeInput", "Compose", "Convert",
  "Dead", "FinalMode", "GroupFirst", "GroupLast", "GroupNext",
  "GroupPrevious", "ModeChange", "NextCandidate", "NonConvert",
  "PreviousCandidate", "Process", "SingleCandidate",
  // Korean, Japanese, Chinese.
  "HangulMode", "HanjaMode", "JunjaMode", "Eisu", "Hankaku", "Hiragana",
  "HiraganaKatakana", "KanaMode", "KanjiMode", "Katakana", "Romaji",
  "Zenkaku", "ZenkakuHankaku",
  // Function keys.
  ...Array.from({ length: 35 }, (_, i) => `F${i + 1}`),
  "Soft1", "Soft2", "Soft3", "Soft4",
  // Multimedia, phone, TV, and browser keys.
  "AppSwitch", "Call", "Camera", "CameraFocus", "EndCall", "GoBack",
  "GoHome", "HeadsetHook", "LastNumberRedial", "Notification", "MannerMode",
  "VoiceDial", "ChannelDown", "ChannelUp", "MediaFastForward", "MediaPause",
  "MediaPlay", "MediaPlayPause", "MediaRecord", "MediaRewind", "MediaStop",
  "MediaTrackNext", "MediaTrackPrevious", "AudioBalanceLeft",
  "AudioBalanceRight", "AudioBassBoostDown", "AudioBassBoostToggle",
  "AudioBassBoostUp", "AudioFaderFront", "AudioFaderRear",
  "AudioSurroundModeNext", "AudioTrebleDown", "AudioTrebleUp",
  "AudioVolumeDown", "AudioVolumeMute", "AudioVolumeUp", "MicrophoneToggle",
  "MicrophoneVolumeDown", "MicrophoneVolumeMute", "MicrophoneVolumeUp",
  "TV", "TVInput", "TVPower", "Exit", "Guide", "Info", "Settings", "Teletext",
  "LaunchCalculator", "LaunchCalendar", "LaunchContacts", "LaunchMail",
  "LaunchMediaPlayer", "LaunchMusicPlayer", "LaunchMyComputer",
  "LaunchPhone", "LaunchScreenSaver", "LaunchSpreadsheet",
  "LaunchWebBrowser", "LaunchWebCam", "LaunchWordProcessor",
  "BrowserBack", "BrowserFavorites", "BrowserForward", "BrowserHome",
  "BrowserRefresh", "BrowserSearch", "BrowserStop",
  "Close", "MailForward", "MailReply", "MailSend", "New", "Open", "Print",
  "Save", "SpellCheck", "Lang1", "Lang2", "Lang3", "Lang4", "Lang5",
  "Dimmer", "Zoom", "ZoomToggle", "Key11", "Key12",
]);
const CONTROL_CHARACTER = /\p{Cc}/u;

/**
 * The text of a keydown whose `key` is a whole string rather than one
 * character, or null when xterm should handle the event itself.
 *
 * Some input methods (GoTiengViet on macOS) erase the typed letters and then
 * deliver the replacement as a single non-composing keydown, e.g. `key` is
 * "ếng". xterm emits only the first character of that, so the rest is lost.
 *
 * Any multi-character string that is not a known key name is text, ASCII
 * included: "eng" and "Tien" are what the method resends after a tone mark is
 * removed. The cost is that a capitalised ASCII word equal to a key name
 * ("End", "Home") is left to xterm, which is never Vietnamese.
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
  if (NAMED_KEYS.has(key) || CONTROL_CHARACTER.test(key)) return null;
  return key;
}
