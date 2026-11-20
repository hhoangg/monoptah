// The footer of the app, like the native footer: a thin bar at the bottom with the Terminal chip on its right (no desktop
// name, no online status). The chip shows only when the desktop advertises `terminal` for this device (a read-only device,
// or a Control device whose "Allow web terminal" is off, never sees it). From 640px a click opens a NEW terminal as a tab
// of the focused pane; on a phone it opens the terminal drawer. A subtle count shows how many shells are running.
import { useRouter } from "@tanstack/react-router";
import { openTab, terminalTab } from "@/features/layout/tabs/paneTree";
import { updateWorkspace } from "@/features/layout/tabs/tabsStore";
import { newTerminalId, useDrawer, useDrawerCommands, useLiveTerminalCount } from "@/features/terminal/sessions";
import { TERMINAL_NAV_LABEL } from "@/features/terminal/copy";
import { cn } from "@/lib/utils";
import { Icon } from "@/ui/Icon";
import { useCapability } from "@/version";

export function FooterBar({ desktopId, tabbed }: { desktopId: string; tabbed: boolean }) {
  const router = useRouter();
  const allowed = useCapability("terminal");
  const count = useLiveTerminalCount();
  const drawer = useDrawer();
  const commands = useDrawerCommands();

  function onClick(): void {
    if (!tabbed) {
      commands.toggleDrawer();
      return;
    }
    const id = newTerminalId();
    updateWorkspace(desktopId, (ws) => openTab(ws, terminalTab(id)));
    void router.navigate({ to: "/d/$desktopId/t/$terminalId", params: { desktopId, terminalId: id } });
  }

  return (
    <footer
      data-testid="app-footer"
      className="flex min-h-[30px] shrink-0 items-center justify-end gap-2.5 border-t border-stroke bg-drawer px-2.5 pr-[max(0.625rem,env(safe-area-inset-right))] pb-[env(safe-area-inset-bottom)] pl-[max(0.625rem,env(safe-area-inset-left))] text-caption text-muted-text"
    >
      {allowed ? (
        <button
          type="button"
          data-testid="terminal-chip"
          aria-label={count > 0 ? `${TERMINAL_NAV_LABEL} (${count} open)` : TERMINAL_NAV_LABEL}
          aria-pressed={tabbed ? undefined : drawer.open}
          onClick={onClick}
          className={cn(
            "inline-flex h-[22px] items-center gap-1.5 rounded-[6px] px-1.5 text-caption hover:bg-selection hover:text-content focus-visible:outline-2 focus-visible:outline-brand pointer-coarse:h-11 pointer-coarse:px-3",
            !tabbed && drawer.open && "text-content",
          )}
        >
          <Icon name="terminal" size={14} />
          <span>{TERMINAL_NAV_LABEL}</span>
          {count > 0 ? (
            <span data-testid="terminal-count" aria-hidden="true" className="rounded-pill bg-fill px-1.5 text-micro font-semibold text-content">
              {count}
            </span>
          ) : null}
        </button>
      ) : null}
    </footer>
  );
}
