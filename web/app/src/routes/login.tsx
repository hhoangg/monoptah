// Route of /login. Owner: WS-S (workstreams.md 3.1).
import { createFileRoute, redirect } from "@tanstack/react-router";
import { LoginScreen } from "@/features/accounts/LoginScreen";
import { meQuery } from "@/features/accounts/queries";
import { postSignInTarget } from "@/features/accounts/redirect";

export const Route = createFileRoute("/login")({
  // `redirect` is where to go after sign-in; the value is untrusted, see features/accounts/redirect.ts.
  validateSearch: (search: Record<string, unknown>): { redirect?: string } =>
    typeof search.redirect === "string" ? { redirect: search.redirect } : {},
  // A visitor who already has a session is sent on instead of seeing the sign-in screen.
  beforeLoad: async ({ context, search }) => {
    let signedIn = false;
    try {
      signedIn = Boolean(await context.queryClient.ensureQueryData({ ...meQuery, retry: false }));
    } catch {
      // 401 (signed out), 403 recovery_only or a network error: stay on the sign-in screen.
    }
    if (signedIn) throw redirect({ href: postSignInTarget(search.redirect) });
  },
  component: Page,
});

function Page() {
  const { redirect } = Route.useSearch();
  return <LoginScreen redirect={redirect} />;
}
