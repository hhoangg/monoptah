// The body of one terminal, in a tab or in the phone drawer: the project chooser, the passkey wait, then the xterm screen
// with, on a phone, the key row. There is no status line and no Copy, Paste or Close toolbar: Cmd/Ctrl+C and V (xterm's
// selection semantics, see xtermScreen.ts) and the context menu copy and paste, and closing the tab (or the drawer's end
// button) closes the shell. Terminal output goes only to xterm; this file renders no output text and holds none.
import { useCallback, useEffect, useRef, useState, useSyncExternalStore } from "react";
import { Button } from "@/ui/button";
import { DropdownMenu, DropdownMenuContent, DropdownMenuItem, DropdownMenuTrigger } from "@/ui/dropdown-menu";
import { Icon } from "@/ui/Icon";
import { Notice } from "@/ui/Notice";
import { useMediaQuery, COARSE_POINTER_QUERY } from "@/ui/useMediaQuery";
import { KeyboardRow } from "./KeyboardRow";
import { applyModifiers, type Modifiers } from "./input";
import type { CreateScreen, TerminalSession } from "./session";
import { TerminalChooser } from "./TerminalChooser";

const NO_MODIFIERS: Modifiers = { ctrl: false, alt: false };

export interface TerminalPaneProps {
  session: TerminalSession;
  /** `drawer` always shows the key row; a tab shows it on touch screens only. */
  variant: "tab" | "drawer";
  /** Overrides the screen factory (tests). */
  createScreen?: CreateScreen | undefined;
}

export function TerminalPane({ session, variant, createScreen }: TerminalPaneProps) {
  const snapshot = useSyncExternalStore(session.controller.subscribe, session.controller.getSnapshot);
  useSyncExternalStore(session.subscribe, session.getVersion);
  const { phase, ended } = snapshot;
  const chooser = phase === "failed" || (phase === "idle" && session.target === null);
  const touch = useMediaQuery(COARSE_POINTER_QUERY);

  if (chooser) {
    return (
      <div data-testid="terminal-pane" data-phase={phase} className="flex min-h-0 flex-1 flex-col overflow-y-auto">
        <TerminalChooser error={snapshot.error} onOpen={(target) => session.open(target)} />
      </div>
    );
  }
  return <Screen session={session} variant={variant} showKeys={variant === "drawer" || touch} ended={ended} phase={phase} inputNotice={snapshot.inputNotice} createScreen={createScreen} />;
}

function Screen({
  session,
  variant,
  showKeys,
  ended,
  phase,
  inputNotice,
  createScreen,
}: {
  session: TerminalSession;
  variant: "tab" | "drawer";
  showKeys: boolean;
  ended: { text: string } | null;
  phase: string;
  inputNotice: string | null;
  createScreen: CreateScreen | undefined;
}) {
  const container = useRef<HTMLDivElement>(null);
  const [modifiers, setModifiers] = useState<Modifiers>(NO_MODIFIERS);
  const modifiersRef = useRef<Modifiers>(NO_MODIFIERS);
  const [flash, setFlash] = useState<string | null>(null);
  const [menu, setMenu] = useState<{ x: number; y: number } | null>(null);

  const updateModifiers = useCallback((next: Modifiers) => {
    modifiersRef.current = next;
    setModifiers(next);
  }, []);

  /** Typed keys and the key row share this path: the sticky modifiers apply to the next key, then clear. */
  const send = useCallback(
    (text: string) => {
      session.controller.input(applyModifiers(text, modifiersRef.current));
      if (modifiersRef.current.ctrl || modifiersRef.current.alt) updateModifiers(NO_MODIFIERS);
    },
    [session, updateModifiers],
  );

  // The screen lives in the session: this view only moves its element in while it is mounted (the tab is the active one).
  useEffect(() => {
    const host = container.current;
    if (!host) return;
    host.appendChild(session.element);
    session.setInput(send);
    session.ensureScreen(createScreen);
    const frame = requestAnimationFrame(() => {
      try {
        session.screen?.fit();
      } catch {
        // Not laid out yet: the observer of the screen fits it once it is.
      }
      session.screen?.focus();
    });
    return () => {
      cancelAnimationFrame(frame);
      session.setInput(null);
      session.element.remove();
    };
  }, [session, send, createScreen]);

  // Focus the screen once it exists (the first mount created it after this effect ran).
  const screen = session.screen;
  useEffect(() => {
    screen?.focus();
  }, [screen]);

  useEffect(() => {
    if (flash === null) return;
    const timer = window.setTimeout(() => setFlash(null), 2500);
    return () => window.clearTimeout(timer);
  }, [flash]);

  async function copy(): Promise<void> {
    const text = session.screen?.getSelection() ?? "";
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
      if (text !== "") session.screen?.paste(text);
      session.screen?.focus();
    } catch {
      setFlash("Paste is not allowed here. Use the keyboard shortcut.");
    }
  }

  const live = ended === null && (phase === "running" || phase === "reconnecting");
  const waiting = phase === "starting" || (phase === "idle" && session.screenError === null);

  return (
    <div data-testid="terminal-pane" data-phase={phase} data-variant={variant} className="relative flex min-h-0 flex-1 flex-col bg-[#131313]">
      {session.screenError ? <Notice variant="error" className="m-3">{session.screenError}</Notice> : null}
      {ended ? (
        <div className="flex flex-wrap items-center gap-3 border-b border-stroke bg-sheet px-3 py-2" data-testid="terminal-ended">
          <p role="status" className="min-w-0 flex-1 text-small">
            Session ended: {ended.text}
          </p>
          <Button type="button" size="sm" onClick={() => session.backToChooser()}>
            New terminal
          </Button>
        </div>
      ) : null}
      {inputNotice ? (
        <p role="status" data-testid="terminal-input-notice" className="border-b border-stroke bg-sheet px-3 py-1.5 text-caption text-caution">
          {inputNotice}
        </p>
      ) : null}
      <div className="relative min-h-0 flex-1">
        <div
          ref={container}
          role="group"
          data-testid="terminal-screen"
          aria-label="Terminal"
          onContextMenu={(event) => {
            event.preventDefault();
            setMenu({ x: event.clientX, y: event.clientY });
          }}
          className="h-full w-full overflow-hidden p-2"
        />
        {waiting ? (
          <div data-testid="terminal-waiting" className="absolute inset-0 flex flex-col items-center justify-center gap-3 bg-[#131313] p-4 text-center">
            <p role="status" className="flex items-center gap-2 text-small text-muted-text">
              <Icon name="fingerprint" size={18} />
              {phase === "starting" ? "Confirm with your passkey to open the terminal..." : "Preparing the terminal..."}
            </p>
            <Button type="button" variant="outline" size="sm" onClick={() => session.backToChooser()}>
              Cancel
            </Button>
          </div>
        ) : null}
        {flash ? (
          <p role="status" className="pointer-events-none absolute right-3 bottom-3 rounded-row border border-stroke bg-sheet px-2.5 py-1 text-caption text-content">
            {flash}
          </p>
        ) : null}
      </div>
      <DropdownMenu open={menu !== null} onOpenChange={(open) => (open ? undefined : setMenu(null))}>
        <DropdownMenuTrigger asChild>
          <span aria-hidden="true" data-testid="terminal-menu-anchor" style={{ position: "fixed", left: menu?.x ?? 0, top: menu?.y ?? 0, width: 0, height: 0 }} />
        </DropdownMenuTrigger>
        <DropdownMenuContent align="start" sideOffset={0} aria-label="Terminal actions" onCloseAutoFocus={(event) => event.preventDefault()}>
          <DropdownMenuItem
            onSelect={() => {
              void copy();
              session.screen?.focus();
            }}
          >
            Copy
          </DropdownMenuItem>
          <DropdownMenuItem onSelect={() => void paste()}>Paste</DropdownMenuItem>
        </DropdownMenuContent>
      </DropdownMenu>
      {showKeys && live ? (
        <KeyboardRow modifiers={modifiers} onModifiers={updateModifiers} onSend={send} applicationCursor={() => session.screen?.applicationCursor() ?? false} onPaste={() => void paste()} />
      ) : null}
    </div>
  );
}
