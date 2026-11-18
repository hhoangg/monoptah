import { OfflineBanner } from "@/features/offline";
import { Link, Outlet, useRouterState } from "@tanstack/react-router";
import { openCommandPalette } from "@/features/layout/palette";
import { Glass } from "./Glass";
import { Icon, type IconName } from "./Icon";
import { useMediaQuery } from "./useMediaQuery";

/** The one `main` landmark of the app: the skip link points at it. Screens never render their own `main`. */
export const MAIN_ID = "main-content";

/** Screens of a visitor who has no session yet: no app navigation. */
const SIGN_IN_FAMILY: readonly string[] = ["/login", "/register", "/recover"];

const NAV: ReadonlyArray<{ to: string; label: string; icon: IconName }> = [
  { to: "/desktops", label: "Desktops", icon: "computer" },
  { to: "/settings", label: "Settings", icon: "settings" },
  { to: "/install", label: "Install", icon: "download" },
];

/** Below this width there is no persistent sidebar (and no keyboard), so the palette needs its own tap target. */
const TABLET_QUERY = "(min-width: 640px)";

/** Shared Wave 0 shell: 44px header on a glass pill, safe-area padding, one scrolling main. */
export function Shell() {
  // The sign-in family gets the brand only. /install stays reachable from the app, so it keeps the nav.
  const publicPage = useRouterState({ select: (state) => SIGN_IN_FAMILY.includes(state.location.pathname.replace(/(.)\/$/, "$1")) });
  // The palette is mounted by the app shell of the /d/:desktopId routes only.
  const inDesktop = useRouterState({ select: (state) => state.location.pathname.startsWith("/d/") });
  const phone = !useMediaQuery(TABLET_QUERY);
  return (
    <div className="flex min-h-dvh flex-col bg-background text-content">
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
      <header className="sticky top-0 z-10 flex min-h-11 items-center gap-2 px-2 pt-[env(safe-area-inset-top)] min-[24rem]:px-5">
        <Glass className="flex min-h-11 min-w-0 flex-1 flex-wrap items-center gap-1 px-2">
          <Link to="/" className="rounded-pill px-3 py-2 text-small font-semibold">
            Monoptah
          </Link>
          {publicPage ? null : (
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
            {NAV.map((item) => (
              <Link
                key={item.to}
                to={item.to}
                aria-label={item.label}
                className="inline-flex size-11 items-center justify-center rounded-pill text-muted-text hover:bg-selection pointer-fine:size-9 [&.active]:text-content"
              >
                <Icon name={item.icon} />
              </Link>
            ))}
          </nav>
          )}
        </Glass>
      </header>
      <OfflineBanner />
      <main id={MAIN_ID} tabIndex={-1} className="flex-1 outline-none pb-[calc(env(safe-area-inset-bottom)+2.5rem)]">
        <Outlet />
      </main>
    </div>
  );
}
