// The terminal itself: the xterm screen (loaded on demand), the status line with Copy, Paste and Close terminal, the ended
// banner and, on touch and narrow layouts, the key row. Terminal output goes only to xterm (`xtermScreen.ts`); this file
// renders no output text and holds none.
import { useCallback, useEffect, useEffectEvent, useRef, useState, useSyncExternalStore } from "react";
import { Button } from "@/ui/button";
import { ConfirmDialog } from "@/ui/ConfirmDialog";
import { Icon } from "@/ui/Icon";
import { Notice } from "@/ui/Notice";
import { useMediaQuery } from "@/ui/useMediaQuery";
import type { TerminalController } from "./controller";
import { KeyboardRow } from "./KeyboardRow";
import { applyModifiers, type Modifiers } from "./input";
import { limitsHint, useNow } from "./time";
import { createXtermScreen, type XtermScreen } from "./xtermScreen";

const NO_MODIFIERS: Modifiers = { ctrl: false, alt: false };
/** The helper row shows on touch screens and on narrow windows. */
const KEYS_QUERY = "(pointer: coarse), (max-width: 639px)";

export interface TerminalViewProps {
  controller: TerminalController;
  /** Called once the screen is attached: the page starts the open flow from here, so the size is known. */
  onScreen: () => void;
  /** After the ended banner: back to the chooser. */
  onLeave: () => void;
  /** Overrides the screen factory (tests). */
  createScreen?: typeof createXtermScreen;
}

export function TerminalView({ controller, onScreen, onLeave, createScreen = createXtermScreen }: TerminalViewProps) {
  const snapshot = useSyncExternalStore(controller.subscribe, controller.getSnapshot);
  const host = useRef<HTMLDivElement>(null);
  const screen = useRef<XtermScreen | null>(null);
  const [failed, setFailed] = useState<string | null>(null);
  const [modifiers, setModifiers] = useState<Modifiers>(NO_MODIFIERS);
  const modifiersRef = useRef<Modifiers>(NO_MODIFIERS);
  const [confirming, setConfirming] = useState(false);
  const [flash, setFlash] = useState<string | null>(null);
  const showKeys = useMediaQuery(KEYS_QUERY);
  const now = useNow(30_000, snapshot.session !== null && snapshot.ended === null);
  // Reads the page's latest target when the screen is ready, without making the screen depend on it.
  const screenReady = useEffectEvent(() => onScreen());

  const updateModifiers = useCallback((next: Modifiers) => {
    modifiersRef.current = next;
    setModifiers(next);
  }, []);

  /** Typed keys and the key row share this path: the sticky modifiers apply to the next key, then clear. */
  const send = useCallback(
    (text: string) => {
      controller.input(applyModifiers(text, modifiersRef.current));
      if (modifiersRef.current.ctrl || modifiersRef.current.alt) updateModifiers(NO_MODIFIERS);
    },
    [controller, updateModifiers],
  );

  useEffect(() => {
    const element = host.current;
    if (!element) return;
    let cancelled = false;
    let created: XtermScreen | null = null;
    createScreen(element, { onData: (data) => send(data), onResize: (cols, rows) => controller.resize(cols, rows) }).then(
      (made) => {
        if (cancelled) return made.dispose();
        created = made;
        screen.current = made;
        controller.attachScreen(made);
        made.focus();
        screenReady();
      },
      () => setFailed("The terminal could not be loaded. Reload the page and try again."),
    );
    return () => {
      cancelled = true;
      controller.detachScreen();
      screen.current = null;
      created?.dispose();
    };
  }, [controller, createScreen, send]);

  useEffect(() => {
    if (flash === null) return;
    const timer = window.setTimeout(() => setFlash(null), 2000);
    return () => window.clearTimeout(timer);
  }, [flash]);

  async function copy(): Promise<void> {
    const text = screen.current?.getSelection() ?? "";
    if (text === "") return setFlash("Select some text first");
    try {
      await navigator.clipboard.writeText(text);
      setFlash("Copied");
    } catch {
      setFlash("Copy is not available here");
    }
  }

  async function paste(): Promise<void> {
    try {
      const text = await navigator.clipboard.readText();
      if (text !== "") screen.current?.paste(text);
      screen.current?.focus();
    } catch {
      setFlash("Paste is not allowed here. Use the keyboard shortcut.");
    }
  }

  const { phase, session, ended } = snapshot;
  const live = ended === null && session !== null;
  const status = phase === "reconnecting" ? "Reconnecting..." : phase === "starting" || phase === "code" || phase === "verifying" ? "Waiting for the code" : phase === "ended" ? "Ended" : "Connected";

  return (
    <div data-testid="terminal-view" data-phase={phase} className="flex min-h-0 flex-1 flex-col overflow-hidden rounded-row border border-stroke">
      <div className="flex flex-wrap items-center gap-x-3 gap-y-1 border-b border-stroke bg-sheet px-3 py-2 text-small">
        <span className="inline-flex items-center gap-1.5 font-medium">
          <Icon name="terminal" size={16} />
          <span data-testid="terminal-cwd" className="min-w-0 truncate">{session?.cwd ?? "Terminal"}</span>
        </span>
        <span data-testid="terminal-status" className="text-muted-text">
          {status}
        </span>
        {live && session ? (
          <span data-testid="terminal-limits" className="hidden min-w-0 truncate text-caption text-muted-text sm:inline">
            {limitsHint(session, now)}
          </span>
        ) : null}
        <span className="ml-auto flex items-center gap-1.5">
          {flash ? (
            <span role="status" className="text-caption text-muted-text">
              {flash}
            </span>
          ) : null}
          <Button type="button" variant="outline" size="sm" onClick={() => void copy()} disabled={!live}>
            Copy
          </Button>
          <Button type="button" variant="outline" size="sm" onClick={() => void paste()} disabled={!live}>
            Paste
          </Button>
          <Button type="button" variant="destructive" size="sm" onClick={() => setConfirming(true)} disabled={!live}>
            Close terminal
          </Button>
        </span>
      </div>
      {failed ? <Notice variant="error" className="m-3">{failed}</Notice> : null}
      {ended ? (
        <div className="flex flex-wrap items-center gap-3 border-b border-stroke bg-sheet px-3 py-2" data-testid="terminal-ended">
          <p role="status" className="min-w-0 flex-1 text-small">
            Session ended: {ended.text}
          </p>
          <Button type="button" size="sm" onClick={onLeave}>
            Open a new terminal
          </Button>
        </div>
      ) : null}
      {snapshot.inputNotice ? (
        <p role="status" data-testid="terminal-input-notice" className="border-b border-stroke bg-sheet px-3 py-1.5 text-caption text-caution">
          {snapshot.inputNotice}
        </p>
      ) : null}
      <div ref={host} role="group" data-testid="terminal-screen" aria-label="Terminal" className="min-h-0 flex-1 overflow-hidden bg-[#131313] p-2" />
      {showKeys && live ? <KeyboardRow modifiers={modifiers} onModifiers={updateModifiers} onSend={send} applicationCursor={() => screen.current?.applicationCursor() ?? false} /> : null}
      <ConfirmDialog
        open={confirming}
        onOpenChange={setConfirming}
        title="Close this terminal?"
        description="This ends the shell on your desktop and everything running in it."
        confirmLabel="Close terminal"
        destructive
        onConfirm={() => {
          setConfirming(false);
          void controller.close();
        }}
      />
    </div>
  );
}
