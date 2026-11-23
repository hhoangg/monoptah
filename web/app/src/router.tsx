import type { QueryClient } from "@tanstack/react-query";
import { createRouter, type RouterHistory } from "@tanstack/react-router";
import { trackHistory } from "@/features/settings/historyLog";
import { queryClient } from "@/lib/queryClient";
import type { RouterContext } from "@/lib/routerContext";
import { RouteError } from "@/ui/RouteError";
import { routeTree } from "./routeTree.gen";

/** Builds the router; tests pass a memory history and their own query client. */
export function createAppRouter(options: { history?: RouterHistory; queryClient?: QueryClient } = {}) {
  const appRouter = createRouter({
    routeTree,
    history: options.history,
    context: { queryClient: options.queryClient ?? queryClient } satisfies RouterContext,
    defaultPreload: "intent",
    defaultErrorComponent: RouteError,
    scrollRestoration: true,
  });
  // One sessionStorage write per navigation: the settings exit button reads where the person came from.
  trackHistory(appRouter);
  return appRouter;
}

export const router = createAppRouter();

declare module "@tanstack/react-router" {
  interface Register {
    router: typeof router;
  }
}
