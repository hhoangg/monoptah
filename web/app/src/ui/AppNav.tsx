import { useRouterState } from "@tanstack/react-router";
import { openCommandPalette } from "@/features/layout/palette";
import { cn } from "@/lib/utils";
import { AccountMenu } from "./AccountMenu";
import { Icon } from "./Icon";
import { useMediaQuery } from "./useMediaQuery";

/** Below this width there is no persistent sidebar (and no keyboard), so the palette needs its own tap target. */
const TABLET_QUERY = "(min-width: 640px)";

/**
 * The app-level controls of a top bar: the avatar menu (Account settings, All desktops, Install app), plus the
 * phone-only Search button on the /d routes. One component, two homes: the top header of the pages outside a desktop,
 * and the right end of the phone's desktop switcher row (from 640px the /d routes use the activity rail instead).
 */
export function AppNav({ className }: { className?: string }) {
  const inDesktop = useRouterState({ select: (state) => state.location.pathname.startsWith("/d/") });
  const phone = !useMediaQuery(TABLET_QUERY);
  return (
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
      <AccountMenu />
    </nav>
  );
}
