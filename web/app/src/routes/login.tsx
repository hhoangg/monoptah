// Route stub of /login. Owner: WS-S (workstreams.md 3.1).
import { createFileRoute } from "@tanstack/react-router";
import { RouteStub } from "@/ui/RouteStub";

export const Route = createFileRoute("/login")({
  // `redirect` is where to go after sign-in; the value is untrusted, see features/accounts/redirect.ts.
  validateSearch: (search: Record<string, unknown>): { redirect?: string } =>
    typeof search.redirect === "string" ? { redirect: search.redirect } : {},
  component: Page,
});

function Page() {
  const params = Route.useParams();
  return <RouteStub route="/login" purpose="Passkey sign-in with a link to /recover." params={params} />;
}
