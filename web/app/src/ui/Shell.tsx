import { OfflineBanner } from "@/features/offline";
import { useQuery } from "@tanstack/react-query";
import { Link, Outlet, useRouterState } from "@tanstack/react-router";
import { ApiError } from "@/features/accounts/api";
import { meQuery } from "@/features/accounts/queries";
import { cn } from "@/lib/utils";
import { Glass } from "./Glass";
import { AppNav } from "./AppNav";
import { TooltipProvider } from "./tooltip";

/** The one `main` landmark of the app: the skip link points at it. Screens never render their own `main`. */
export const MAIN_ID = "main-content";

/** Screens of a visitor who has no session yet: no app navigation. */
const SIGN_IN_FAMILY: readonly string[] = ["/login", "/register", "/recover"];

/** Shared Wave 0 shell: 44px header on a glass pill (outside /d routes), safe-area padding, one scrolling main. */
export function Shell() {
  const pathname = useRouterState({ select: (state) => state.location.pathname.replace(/(.)\/$/, "$1") });
  const signInFamily = SIGN_IN_FAMILY.includes(pathname);
  // The /d/:desktopId routes have no header: their nav icons sit at the right end of the desktop switcher row.
  const inDesktop = pathname.startsWith("/d/");
  // /install is public: a visitor without a session sees the brand only (the nav entries would bounce to /login).
  const me = useQuery({ ...meQuery, enabled: pathname === "/install", retry: false, networkMode: "always" });
  // Brand only until /api/me succeeds. A signed-in visitor who is merely offline (a network error, not a 401) keeps the nav.
  const signedOut = me.isError && me.error instanceof ApiError && (me.error.status === 401 || me.error.status === 403);
  const publicPage = signInFamily || (pathname === "/install" && (me.isPending || signedOut));
  return (
    <TooltipProvider delayDuration={300}>
    {/* /d routes: a fixed-height flex column (header, switcher and banner shrink-0, main fills the rest) so no screen needs 100dvh math and the body never scrolls. */}
    <div className={cn("flex flex-col bg-background text-content", inDesktop ? "relative h-dvh overflow-hidden" : "min-h-dvh")}>
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
      {inDesktop ? null : (
        <header className="sticky top-0 z-10 flex min-h-11 shrink-0 items-center gap-2 pt-[env(safe-area-inset-top)] pr-[max(0.5rem,env(safe-area-inset-right))] pl-[max(0.5rem,env(safe-area-inset-left))] min-[24rem]:pr-[max(1.25rem,env(safe-area-inset-right))] min-[24rem]:pl-[max(1.25rem,env(safe-area-inset-left))]">
          {publicPage ? (
            <Link to="/" className="rounded-pill px-3 py-2 text-small font-semibold">
              Monoptah
            </Link>
          ) : (
            <Glass className="flex min-h-11 min-w-0 flex-1 flex-wrap items-center gap-1 px-2">
              <Link to="/" className="rounded-pill px-3 py-2 text-small font-semibold">
                Monoptah
              </Link>
              <AppNav className="ml-auto" />
            </Glass>
          )}
        </header>
      )}
      {/* On /d the banner sits under the desktop switcher row (DesktopTopBar owns the top safe-area inset there). */}
      {inDesktop ? null : <OfflineBanner />}
      <main
        id={MAIN_ID}
        tabIndex={-1}
        className={cn("outline-none", inDesktop ? "relative flex min-h-0 flex-1 flex-col overflow-y-auto" : "flex-1 pb-[calc(env(safe-area-inset-bottom)+2.5rem)]")}
      >
        <Outlet />
      </main>
    </div>
    </TooltipProvider>
  );
}
