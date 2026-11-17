// Root `beforeLoad` auth guard hook (architecture.md 3.1 "Guards").
//
// NOT implemented in Wave 0. W-02 (WS-S) replaces the body: call
// `queryClient.ensureQueryData(['me'])` and, on a 401, throw
// `redirect({ to: '/login', search: { redirect: location.href } })` for every route that is not public.
//
// Fragments to handle in W-02 / W-03 (read in the route components, never sent to the server):
//   `#invite=CODE` on /register (W-02) and `#fp=<hex>` on /pair/:code and /desktops/:id/verify (W-03).
// `#fp` is saved to sessionStorage before any redirect to /login and restored after sign-in.
import type { ParsedLocation } from "@tanstack/react-router";
import type { QueryClient } from "@tanstack/react-query";

export interface AuthGuardArgs {
  location: ParsedLocation;
  context: { queryClient: QueryClient };
}

export type AuthGuard = (args: AuthGuardArgs) => void | Promise<void>;

// TODO(W-02): implement the guard. Wave 0 lets every route through.
export const authGuard: AuthGuard = () => {};
