import { Link, useRouterState } from "@tanstack/react-router";
import { useStandalone } from "@/features/settings/standalone";
import { cn } from "@/lib/utils";
import { DropdownMenu, DropdownMenuContent, DropdownMenuItem, DropdownMenuTrigger } from "./dropdown-menu";
import { Icon, type IconName } from "./Icon";
import { Tooltip, TooltipContent, TooltipProvider, TooltipTrigger } from "./tooltip";

const ENTRIES: ReadonlyArray<{ to: string; label: string; icon: IconName }> = [
  { to: "/settings/profile", label: "Settings", icon: "account" },
  { to: "/settings/desktops", label: "Desktops", icon: "computer" },
  { to: "/settings/install", label: "Install app", icon: "download" },
];

/**
 * The one account control of the app: an avatar button that opens a menu with Settings, Desktops and the Install app
 * guide (hidden once the app is installed). All three are pages of the settings shell (/settings/*). It is the only app-level entry of the top header, the phone
 * switcher bar and the activity rail, so the Desktops and Install icons no longer sit beside it.
 */
export function AccountMenu({
  side = "bottom",
  className,
  tooltipSide,
}: {
  /** Which side of the button the menu opens on: "right" from the activity rail, "bottom" from a top bar. */
  side?: "bottom" | "right";
  className?: string;
  tooltipSide?: "bottom" | "right";
}) {
  const pathname = useRouterState({ select: (state) => state.location.pathname.replace(/(.)\/$/, "$1") });
  const standalone = useStandalone();
  const entries = ENTRIES.filter((entry) => !(entry.to === "/settings/install" && standalone));
  const current = pathname.startsWith("/settings") || pathname.startsWith("/admin");
  return (
    <TooltipProvider delayDuration={300}>
      <DropdownMenu>
        <Tooltip>
          <TooltipTrigger asChild>
            <DropdownMenuTrigger asChild>
              <button
                type="button"
                aria-label="Account"
                data-current={current ? "true" : undefined}
                className={cn(
                  "inline-flex size-11 shrink-0 items-center justify-center rounded-pill text-muted-text hover:bg-selection pointer-fine:size-9 data-[current=true]:bg-selection data-[current=true]:text-content data-[state=open]:bg-selection data-[state=open]:text-content",
                  className,
                )}
              >
                <Icon name="account" />
              </button>
            </DropdownMenuTrigger>
          </TooltipTrigger>
          <TooltipContent side={tooltipSide ?? side} className="hidden pointer-fine:block">
            Account
          </TooltipContent>
        </Tooltip>
        <DropdownMenuContent align="end" side={side}>
          {entries.map((entry) => (
            <DropdownMenuItem key={entry.to} asChild>
              <Link to={entry.to}>
                <Icon name={entry.icon} size={18} />
                {entry.label}
              </Link>
            </DropdownMenuItem>
          ))}
        </DropdownMenuContent>
      </DropdownMenu>
    </TooltipProvider>
  );
}
