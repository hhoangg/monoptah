// Route of /pair/:code. Owner: WS-S (workstreams.md 3.1). The root guard sends a signed-out visitor to
// /login and keeps the code (path) and `#fp` (sessionStorage); see features/accounts/redirect.ts.
import { createFileRoute } from "@tanstack/react-router";
import { PairScreen } from "@/features/accounts/pair/PairScreen";

export const Route = createFileRoute("/pair/$code")({
  component: Page,
});

function Page() {
  const { code } = Route.useParams();
  return <PairScreen code={code} />;
}
