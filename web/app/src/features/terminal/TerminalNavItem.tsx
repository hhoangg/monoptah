// The entry of the terminal in the workspace list (the phone drawer and the sidebar), only when the desktop advertises
// `terminal` for this device: a read-only device, or a Control device whose "Allow web terminal" is off, never sees it.
import { Link } from "@tanstack/react-router";
import { Icon } from "@/ui/Icon";
import { useCapability } from "@/version";
import { TERMINAL_NAV_LABEL } from "./copy";

export function TerminalNavItem({ desktopId, onNavigate }: { desktopId: string; onNavigate?: (() => void) | undefined }) {
  const enabled = useCapability("terminal");
  if (!enabled) return null;
  return (
    <Link to="/d/$desktopId/terminal" params={{ desktopId }} onClick={onNavigate} className="flex min-h-11 items-center gap-3 rounded-row px-2.5 text-row hover:bg-content-a04 [&.active]:bg-selection">
      <Icon name="terminal" size={20} />
      {TERMINAL_NAV_LABEL}
    </Link>
  );
}
