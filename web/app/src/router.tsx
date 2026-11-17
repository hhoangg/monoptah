import type { QueryClient } from "@tanstack/react-query";
import { createRouter, type RouterHistory } from "@tanstack/react-router";
import { queryClient } from "@/lib/queryClient";
import type { RouterContext } from "@/lib/routerContext";
import { routeTree } from "./routeTree.gen";

/** Builds the router; tests pass a memory history and their own query client. */
export function createAppRouter(options: { history?: RouterHistory; queryClient?: QueryClient } = {}) {
  return createRouter({
    routeTree,
    history: options.history,
    context: { queryClient: options.queryClient ?? queryClient } satisfies RouterContext,
    defaultPreload: "intent",
    scrollRestoration: true,
  });
}

export const router = createAppRouter();

declare module "@tanstack/react-router" {
  interface Register {
    router: typeof router;
  }
}
