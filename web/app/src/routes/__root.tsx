// Root route: the shared shell and the auth guard hook. Owner: WS-S.
import { createRootRouteWithContext } from "@tanstack/react-router";
import { authGuard } from "@/features/accounts/authGuard";
import type { RouterContext } from "@/lib/routerContext";
import { Shell } from "@/ui/Shell";

export const Route = createRootRouteWithContext<RouterContext>()({
  // Public routes pass; everything else needs a session (features/accounts/authGuard.ts).
  beforeLoad: ({ location, context }) => authGuard({ location, context }),
  component: Shell,
});
