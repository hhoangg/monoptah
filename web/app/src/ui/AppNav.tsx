import { Link, useRouterState } from "@tanstack/react-router";
import { openCommandPalette } from "@/features/layout/palette";
import { useStandalone } from "@/features/settings/standalone";
import { cn } from "@/lib/utils";
import { DropdownMenu, DropdownMenuContent, DropdownMenuItem, DropdownMenuTrigger } from "./dropdown-menu";
import { Icon, type IconName } from "./Icon";
import { Tooltip, TooltipContent, TooltipProvider, TooltipTrigger } from "./tooltip";
import { useMediaQuery } from "./useMediaQuery";

const NAV: ReadonlyArray<{ to: string; label: string; icon: IconName; /** Extra path prefix that also marks this entry as the current one. */ alsoActiveFor?: string }> = [
  { to: "/desktops", label: "Desktops", icon: "computer" },
  { to: "/settings", label: "Account", icon: "account", alsoActiveFor: "/admin" },
  { to: "/install", label: "Install", icon: "download" },
];

const navLink =
  "inline-flex size-11 items-center justify-center rounded-pill text-muted-text hover:bg-selection pointer-fine:size-9 [&.active]:bg-selection [&.active]:text-content data-[current=true]:bg-selection data-[current=true]:text-content";

/** Below this width there is no persistent sidebar (and no keyboard), so the palette needs its own tap target. */
const TABLET_QUERY = "(min-width: 640px)";

/** A phone (under 640px): the /d switcher row is too short for Search and three icons beside the desktop name. */
const PHONE_QUERY = "(max-width: 39.9375rem)";

/**
 * The three app-level entries (Desktops, Account, Install) as icon links, plus the phone-only Search button on the
 * /d routes. One component, two homes: the top header of the pages outside a desktop, and the right end of the
 * desktop switcher row inside one (the /d routes have no header of their own). On a phone inside a desktop the row
 * keeps Search and Desktops (the way out) and folds Account and Install into one "More" menu, so the desktop name
 * keeps its room.
 */
export function AppNav({ className }: { className?: string }) {
  const pathname = useRouterState({ select: (state) => state.location.pathname.replace(/(.)\/$/, "$1") });
  const inDesktop = pathname.startsWith("/d/");
  const phone = !useMediaQuery(TABLET_QUERY);
  const standalone = useStandalone();
  const nav = NAV.filter((item) => !(item.to === "/install" && standalone));
  const narrow = useMediaQuery(PHONE_QUERY);
  const fold = inDesktop && narrow;
  const shown = fold ? nav.filter((item) => item.to === "/desktops") : nav;
  const folded = fold ? nav.filter((item) => item.to !== "/desktops") : [];
  const moreCurrent = folded.some((item) => item.to === pathname || (item.alsoActiveFor !== undefined && pathname.startsWith(item.alsoActiveFor)));
  return (
    <TooltipProvider delayDuration={300}>
    <nav aria-label="Main" className={cn("flex items-center gap-1", className)}>
      {inDesktop && phone ? (
        <button
          type="button"
          aria-label="Search"
          onClick={openCommandPalette}
          className="inline-flex size-11 items-center justify-center rounded-pill text-muted-text hover:bg-selection pointer-fine:size-9"
        >
          <Icon name="search" />
        </button>
      ) : null}
      {shown.map((item) => (
        <Tooltip key={item.to}>
          <TooltipTrigger asChild>
            <Link
              to={item.to}
              aria-label={item.label}
              data-current={item.alsoActiveFor !== undefined && pathname.startsWith(item.alsoActiveFor) ? "true" : undefined}
              className={navLink}
            >
              <Icon name={item.icon} />
            </Link>
          </TooltipTrigger>
          <TooltipContent className="hidden pointer-fine:block">{item.label}</TooltipContent>
        </Tooltip>
      ))}
      {folded.length > 0 ? (
        <DropdownMenu>
          <DropdownMenuTrigger asChild>
            <button
              type="button"
              aria-label="More"
              data-current={moreCurrent ? "true" : undefined}
              aria-current={moreCurrent ? "page" : undefined}
              className={navLink}
            >
              <Icon name="more" />
            </button>
          </DropdownMenuTrigger>
          <DropdownMenuContent align="end">
            {folded.map((item) => (
              <DropdownMenuItem key={item.to} asChild>
                <Link to={item.to}>
                  <Icon name={item.icon} size={18} />
                  {item.label}
                </Link>
              </DropdownMenuItem>
            ))}
          </DropdownMenuContent>
        </DropdownMenu>
      ) : null}
    </nav>
    </TooltipProvider>
  );
}
