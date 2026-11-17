// Route of /login. Owner: WS-S (workstreams.md 3.1).
import { createFileRoute } from "@tanstack/react-router";
import { LoginScreen } from "@/features/accounts/LoginScreen";

export const Route = createFileRoute("/login")({
  // `redirect` is where to go after sign-in; the value is untrusted, see features/accounts/redirect.ts.
  validateSearch: (search: Record<string, unknown>): { redirect?: string } =>
    typeof search.redirect === "string" ? { redirect: search.redirect } : {},
  component: Page,
});

function Page() {
  const { redirect } = Route.useSearch();
  return <LoginScreen redirect={redirect} />;
}
