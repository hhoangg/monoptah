import { invoke } from "@tauri-apps/api/core";
import { getCurrentWebviewWindow } from "@tauri-apps/api/webviewWindow";
import { useEffect, useRef, useState } from "react";
import { createPortal } from "react-dom";
import { IS_MAC } from "../../platform/tauri/platform";
import { LAYER } from "../../shared/lib/layers";
import {
  isQuitHotkey,
  quitGatePress,
  type QuitGateState,
} from "../model/quitGate";

/** What Rust forwards once its monitor has swallowed the macOS quit hotkey. */
const QUIT_GATE_PRESS = "quit_gate_press";

export const QUIT_GATE_LABEL = IS_MAC ? "Hold ⌘Q to Quit" : "Hold Ctrl+Q to Quit";

/**
 * Catches the quit hotkey and asks for it twice — held down, or pressed again —
 * before letting the app go. The press arrives from the macOS event monitor, or
 * straight off the keyboard everywhere else.
 */
export function QuitGateNotice() {
  const [armed, setArmed] = useState(false);
  const state = useRef<QuitGateState>(null);

  useEffect(() => {
    let expiry = 0;

    const apply = (next: QuitGateState) => {
      state.current = next;
      setArmed(!!next);
      window.clearTimeout(expiry);
      if (!next) return;
      // Only ever the timer for the state just applied: every apply clears the
      // one before it, so firing always means this notice's window ran out.
      expiry = window.setTimeout(() => {
        state.current = null;
        setArmed(false);
      }, next.expiresAt - Date.now());
    };

    const press = (repeat: boolean) => {
      const step = quitGatePress(state.current, Date.now(), repeat);
      apply(step.state);
      if (step.quit) void invoke("quit_now").catch(() => undefined);
    };

    // macOS: the menu accelerator would swallow the key before the webview
    // sees it, so Rust intercepts it and hands it over. Scoped to this window
    // on purpose — a global `listen` is registered as `Any`, which Tauri
    // matches whoever the emitter aimed at, so every window would light up.
    const unlisten = IS_MAC
      ? getCurrentWebviewWindow().listen<{ repeat: boolean }>(
          QUIT_GATE_PRESS,
          (event) => press(!!event.payload?.repeat),
        )
      : null;

    const onKeyDown = (event: KeyboardEvent) => {
      if (IS_MAC || !isQuitHotkey(event, false)) return;
      // Ctrl+Q is XON inside a terminal; a shell waiting on it must keep it.
      if ((event.target as Element | null)?.closest?.(".monocode-terminal")) {
        return;
      }
      event.preventDefault();
      press(event.repeat);
    };

    if (!IS_MAC) window.addEventListener("keydown", onKeyDown, true);
    return () => {
      window.clearTimeout(expiry);
      window.removeEventListener("keydown", onKeyDown, true);
      void unlisten?.then((off) => off()).catch(() => undefined);
    };
  }, []);

  if (!armed) return null;

  return createPortal(
    <div
      role="status"
      aria-live="polite"
      style={{ zIndex: LAYER.toast }}
      className="pointer-events-none fixed inset-x-0 top-1/2 flex -translate-y-1/2 justify-center"
    >
      <div className="rounded-xl bg-black/75 px-5 py-3 text-[13px] font-medium text-white shadow-lg backdrop-blur-sm">
        {QUIT_GATE_LABEL}
      </div>
    </div>,
    document.body,
  );
}
