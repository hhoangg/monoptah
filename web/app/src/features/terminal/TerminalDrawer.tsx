// The terminal on a phone (< 640px): a bottom drawer of about 55% of the screen, opened from the footer chip. It is NOT
// modal: no scrim, no focus trap, the workspace above it stays usable. Its header names the project (with the green dot),
// toggles Fullscreen / Shrink and closes the drawer (the shell keeps running). Tapping the project name opens the switcher
// row (always shown with several terminals): pick a terminal, add one, or end one (with the confirm). Dragging the handle up goes fullscreen, down shrinks or closes. Safe-area insets are respected.
import { useEffect, useRef, useState, type PointerEvent } from "react";
import { cn } from "@/lib/utils";
import { useLinkConnection, useLinkDesktopId } from "@/link";
import { Icon } from "@/ui/Icon";
import { TerminalCloseConfirm } from "./TerminalCloseConfirm";
import { TerminalPane } from "./TerminalPane";
import { ensureSession, guardTerminalClose, terminalTitle, useDrawer, useDrawerCommands, useTerminalSession, useTerminalsVersion } from "./sessions";

const DRAG_PX = 48;
const headerButton = "inline-flex size-11 shrink-0 items-center justify-center rounded-pill text-content hover:bg-selection focus-visible:outline-2 focus-visible:outline-brand";

export function TerminalDrawer() {
  const drawer = useDrawer();
  const commands = useDrawerCommands();
  const connection = useLinkConnection();
  const desktopId = useLinkDesktopId();
  useTerminalsVersion();
  // Terminals of the drawer from before a reload get their sessions (and so their reattach) as soon as the drawer shows.
  useEffect(() => {
    if (drawer.open) for (const id of drawer.ids) ensureSession(connection, desktopId, id);
  }, [drawer.open, drawer.ids, connection, desktopId]);
  const drag = useRef<number | null>(null);
  const [switcherOpen, setSwitcherOpen] = useState(false);

  if (!drawer.open || !drawer.active) return null;
  const active = drawer.active;
  const full = drawer.fullscreen;
  const title = terminalTitle(connection, active);
  const project = title.title === "Terminal" ? null : title.title.replace(/^Terminal - /, "");

  const showSwitcher = switcherOpen || drawer.ids.length > 1;

  const close = (): void => {
    commands.closeDrawer();
    // Give the focus back to the chip that opened the drawer.
    requestAnimationFrame(() => document.querySelector<HTMLElement>('[data-testid="terminal-chip"]')?.focus());
  };

  const onPointerDown = (event: PointerEvent): void => {
    drag.current = event.clientY;
    event.currentTarget.setPointerCapture?.(event.pointerId);
  };
  const onPointerUp = (event: PointerEvent): void => {
    const start = drag.current;
    drag.current = null;
    if (start === null) return;
    const dy = event.clientY - start;
    if (dy < -DRAG_PX) commands.setFullscreen(true);
    else if (dy > DRAG_PX) {
      if (full) commands.setFullscreen(false);
      else close();
    }
    else if (Math.abs(dy) < 8) commands.setFullscreen(!full);
  };

  return (
    <section
      role="region"
      aria-label="Terminal drawer"
      data-testid="terminal-drawer"
      data-fullscreen={full}
      className={cn(
        "fixed inset-x-0 bottom-0 z-40 flex flex-col overflow-hidden bg-sheet text-content",
        full ? "top-0 h-dvh pt-[env(safe-area-inset-top)]" : "h-[55dvh] rounded-t-sheet border-t border-stroke",
        "pr-[env(safe-area-inset-right)] pb-[env(safe-area-inset-bottom)] pl-[env(safe-area-inset-left)]",
      )}
    >
      {full ? null : (
        <button
          type="button"
          aria-label="Drag to resize the terminal, or press to go fullscreen"
          data-testid="terminal-handle"
          onPointerDown={onPointerDown}
          onPointerUp={onPointerUp}
          onPointerCancel={() => (drag.current = null)}
          onClick={(event) => {
            // A keyboard activation (no pointer): toggle like a tap.
            if (event.detail === 0) commands.setFullscreen(!full);
          }}
          className="flex h-7 w-full shrink-0 touch-none items-center justify-center"
        >
          <span aria-hidden="true" className="h-[5px] w-9 rounded-[2.5px] bg-faint" />
        </button>
      )}
      <header className="flex shrink-0 items-center gap-1 px-3">
        <Icon name="terminal" size={20} className="shrink-0 text-muted-text" />
        <h2 className="min-w-0 flex-1 text-h font-semibold" data-testid="terminal-drawer-title">
          <button
            type="button"
            aria-expanded={showSwitcher}
            aria-controls="terminal-switcher"
            aria-label={`${project ?? "Terminal"}, show terminals`}
            onClick={() => setSwitcherOpen(!switcherOpen)}
            className="flex min-h-11 max-w-full min-w-0 items-center gap-2 text-left focus-visible:outline-2 focus-visible:outline-brand"
          >
            <span className="min-w-0 truncate">{project ?? "Terminal"}</span>
            {project ? <span role="img" aria-label={title.connected ? "Connected" : "Not connected"} data-testid="terminal-dot" data-state={title.connected ? "on" : "off"} className={cn("size-2 shrink-0 rounded-full", title.connected ? "bg-additions" : "bg-faint")} /> : null}
          </button>
        </h2>
        <button
          type="button"
          aria-pressed={full}
          onClick={() => commands.setFullscreen(!full)}
          className="inline-flex min-h-11 shrink-0 items-center gap-1.5 rounded-pill bg-fill px-3 text-small hover:bg-selection focus-visible:outline-2 focus-visible:outline-brand"
        >
          <Icon name={full ? "exitFullscreen" : "fullscreen"} size={16} />
          {full ? "Shrink" : "Fullscreen"}
        </button>
        <button type="button" aria-label="Close drawer" onClick={close} className={headerButton}>
          <Icon name="close" size={20} />
        </button>
      </header>
      {showSwitcher ? (
        <div id="terminal-switcher" data-testid="terminal-switcher" className="flex shrink-0 items-center gap-1.5 overflow-x-auto px-3 pb-1.5 [scrollbar-width:none]">
          <div role="tablist" aria-label="Terminals" className="flex gap-1.5">
            {drawer.ids.map((id, index) => {
              const info = terminalTitle(connection, id);
              const selected = id === active;
              const name = info.title === "Terminal" ? `Terminal ${index + 1}` : info.title.replace(/^Terminal - /, "");
              return (
                <div key={id} className={cn("inline-flex shrink-0 items-center rounded-pill", selected ? "bg-content text-background" : "bg-fill text-content")}>
                  <button type="button" role="tab" aria-selected={selected} onClick={() => commands.activate(id)} className="inline-flex min-h-11 items-center gap-1.5 rounded-l-pill pr-1 pl-3 text-small">
                    <span className="max-w-32 truncate">{name}</span>
                    {info.title === "Terminal" ? null : <span aria-hidden="true" className={cn("size-1.5 rounded-full", info.connected ? "bg-additions" : "bg-faint")} />}
                  </button>
                  <button type="button" aria-label={`End ${name}`} onClick={() => guardTerminalClose(connection, [id], () => undefined)} className="inline-flex size-11 items-center justify-center rounded-r-pill">
                    <Icon name="close" size={14} />
                  </button>
                </div>
              );
            })}
          </div>
          <button type="button" aria-label="New terminal" onClick={commands.addToDrawer} className={headerButton}>
            <Icon name="add" size={20} />
          </button>
        </div>
      ) : null}
      <DrawerBody key={active} id={active} />
      <TerminalCloseConfirm />
    </section>
  );
}

function DrawerBody({ id }: { id: string }) {
  const session = useTerminalSession(id);
  return <TerminalPane session={session} variant="drawer" />;
}
