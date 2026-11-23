import { useQuery } from "@tanstack/react-query";
import { useRouterState } from "@tanstack/react-router";
import { useEffect, type ReactNode } from "react";
import { meQuery } from "@/features/accounts/queries";
import { cn } from "@/lib/utils";
import { SettingsNav } from "./SettingsNav";
import { useScrollRequests } from "./scrollRequest";

const SCROLL_RETRY_MS = 100;
const SCROLL_GIVE_UP_MS = 3000;

/**
 * Scrolls to the element a `#anchor` in the URL names. The router does this once when the route renders, which is too
 * early for a card that appears after its data loads, so this keeps looking for a short while. It runs once per
 * navigation (each one has its own history key; a pick of the place the URL is already at asks for it again) and uses no
 * animation when the person asked for reduced motion. The router does not expose the history action cheaply, so going
 * back to a `#anchor` entry scrolls to it again rather than restoring the old scroll position.
 */
function useScrollToHash(pathname: string, hash: string, locationKey: string | undefined) {
  // A pick of the place the router is already at changes none of the above (see scrollRequest.ts).
  const requests = useScrollRequests();
  useEffect(() => {
    if (hash === "") return undefined;
    const reduced = typeof window.matchMedia === "function" && window.matchMedia("(prefers-reduced-motion: reduce)").matches;
    const started = Date.now();
    let timer: ReturnType<typeof setTimeout> | undefined;
    const attempt = () => {
      const target = document.getElementById(hash);
      if (target) {
        target.scrollIntoView({ behavior: reduced ? "auto" : "smooth", block: "start" });
        return;
      }
      if (Date.now() - started < SCROLL_GIVE_UP_MS) timer = setTimeout(attempt, SCROLL_RETRY_MS);
    };
    attempt();
    return () => clearTimeout(timer);
  }, [pathname, hash, locationKey, requests]);
}

/**
 * The one settings shell (route root /settings). From 1024px: the nav as a sticky left column and the page on the right.
 * Below that only one shows, decided by CSS from the path (no JS media query): on /settings the nav is the list page
 * (it fills the width, under the page's own "Settings" header, which comes first in the DOM), on a sub page the nav is
 * hidden and the page shows its back link to that list. On the list page the content comes before the nav in the DOM (its
 * h1 reads first); on every other page the nav comes first, so Tab follows the visual order from 1024px. The wide layout
 * puts the nav in the left column with `order-first`. Both children keep a key, so changing the order does not remount them.
 */
export function SettingsShell({ children }: { children: ReactNode }) {
  const me = useQuery(meQuery);
  const pathname = useRouterState({ select: (state) => state.location.pathname.replace(/(.)\/$/, "$1") });
  const hash = useRouterState({ select: (state) => state.location.hash });
  const locationKey = useRouterState({ select: (state) => state.location.state.__TSR_key });
  const isList = pathname === "/settings";
  useScrollToHash(pathname, hash, locationKey);
  const gutters = "px-[max(1.25rem,env(safe-area-inset-left))] pr-[max(1.25rem,env(safe-area-inset-right))]";
  const content = (
    <div key="content" className="min-w-0">
      {children}
    </div>
  );
  // The scroll container gets padding (and a matching negative margin) so the 2px focus outlines of its children are not clipped.
  const nav = (
    <aside
      key="nav"
      className={cn(
        isList ? "block" : "hidden",
        "lg:order-first lg:block lg:sticky lg:top-[calc(env(safe-area-inset-top)+2.25rem)] lg:-m-1 lg:max-h-[calc(100dvh-env(safe-area-inset-top)-3.25rem)] lg:overflow-y-auto lg:overscroll-contain lg:p-1",
      )}
    >
      <SettingsNav admin={me.data?.user.role === "admin"} />
    </aside>
  );
  return (
    <div
      className={cn(
        "mx-auto flex w-full max-w-[760px] flex-col gap-6 pt-6 pb-[calc(env(safe-area-inset-bottom)+1rem)] sm:pt-10 lg:grid lg:max-w-[1040px] lg:grid-cols-[15rem_minmax(0,760px)] lg:items-start lg:gap-10",
        gutters,
      )}
    >
      {isList ? [content, nav] : [nav, content]}
    </div>
  );
}
