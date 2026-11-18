import { OfflineBanner } from "@/features/offline";
import { useQuery } from "@tanstack/react-query";
import { Link, Outlet, useRouterState } from "@tanstack/react-router";
import { meQuery } from "@/features/accounts/queries";
import { openCommandPalette } from "@/features/layout/palette";
import { useStandalone } from "@/features/settings/standalone";
import { cn } from "@/lib/utils";
import { Glass } from "./Glass";
import { Icon, type IconName } from "./Icon";
import { Tooltip, TooltipContent, TooltipProvider, TooltipTrigger } from "./tooltip";
import { useMediaQuery } from "./useMediaQuery";

/** The one `main` landmark of the app: the skip link points at it. Screens never render their own `main`. */
export const MAIN_ID = "main-content";

/** Screens of a visitor who has no session yet: no app navigation. */
const SIGN_IN_FAMILY: readonly string[] = ["/login", "/register", "/recover"];

const NAV: ReadonlyArray<{ to: string; label: string; icon: IconName; /** Extra path prefix that also marks this entry as the current one. */ alsoActiveFor?: string }> = [
  { to: "/desktops", label: "Desktops", icon: "computer" },
  { to: "/settings", label: "Account", icon: "settings", alsoActiveFor: "/admin" },
  { to: "/install", label: "Install", icon: "download" },
];

const navLink =
  "inline-flex size-11 items-center justify-center rounded-pill text-muted-text hover:bg-selection pointer-fine:size-9 [&.active]:bg-selection [&.active]:text-content data-[current=true]:bg-selection data-[current=true]:text-content";

/** Below this width there is no persistent sidebar (and no keyboard), so the palette needs its own tap target. */
const TABLET_QUERY = "(min-width: 640px)";

/** Shared Wave 0 shell: 44px header on a glass pill, safe-area padding, one scrolling main. */
export function Shell() {
  const pathname = useRouterState({ select: (state) => state.location.pathname.replace(/(.)\/$/, "$1") });
  const signInFamily = SIGN_IN_FAMILY.includes(pathname);
  // The palette is mounted by the app shell of the /d/:desktopId routes only.
  const inDesktop = pathname.startsWith("/d/");
  const phone = !useMediaQuery(TABLET_QUERY);
  const standalone = useStandalone();
  // /install is public: a visitor without a session sees the brand only (the nav entries would bounce to /login).
  const me = useQuery({ ...meQuery, enabled: pathname === "/install", retry: false });
  const publicPage = signInFamily || (pathname === "/install" && me.isError);
  const nav = NAV.filter((item) => !(item.to === "/install" && standalone));
  return (
    <TooltipProvider delayDuration={300}>
    {/* /d routes: a fixed-height flex column (header, switcher and banner shrink-0, main fills the rest) so no screen needs 100dvh math and the body never scrolls. */}
    <div className={cn("flex flex-col bg-background text-content", inDesktop ? "h-dvh overflow-hidden" : "min-h-dvh")}>
      {/* First focusable element (design-system.md 9): visible on focus only; moves focus to the main content. */}
      <a
        href={`#${MAIN_ID}`}
        onClick={(event) => {
          event.preventDefault();
          document.getElementById(MAIN_ID)?.focus();
        }}
        className="sr-only z-20 rounded-pill bg-background px-4 py-2 text-small font-semibold text-content focus:not-sr-only focus:fixed focus:top-2 focus:left-2"
      >
        Skip to content
      </a>
      <header
        className={cn(
          "z-10 flex min-h-11 shrink-0 items-center gap-2 pt-[env(safe-area-inset-top)] pr-[max(0.5rem,env(safe-area-inset-right))] pl-[max(0.5rem,env(safe-area-inset-left))] min-[24rem]:pr-[max(1.25rem,env(safe-area-inset-right))] min-[24rem]:pl-[max(1.25rem,env(safe-area-inset-left))]",
          inDesktop ? "" : "sticky top-0",
        )}
      >
        {publicPage ? (
          <Link to="/" className="rounded-pill px-3 py-2 text-small font-semibold">
            Monoptah
          </Link>
        ) : (
          <Glass className="flex min-h-11 min-w-0 flex-1 flex-wrap items-center gap-1 px-2">
            <Link to="/" className="rounded-pill px-3 py-2 text-small font-semibold">
              Monoptah
            </Link>
            <nav aria-label="Main" className="ml-auto flex items-center gap-1">
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
              {nav.map((item) => (
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
            </nav>
          </Glass>
        )}
      </header>
      <OfflineBanner />
      <main
        id={MAIN_ID}
        tabIndex={-1}
        className={cn("outline-none", inDesktop ? "flex min-h-0 flex-1 flex-col overflow-y-auto" : "flex-1 pb-[calc(env(safe-area-inset-bottom)+2.5rem)]")}
      >
        <Outlet />
      </main>
    </div>
    </TooltipProvider>
  );
}
