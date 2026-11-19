// Route of /desktops/:id/verify. Owner: WS-M (workstreams.md 3.1). Re-pins the desktop key after a key
// change by comparing the `#fp` fragment of the link shown on the desktop (R-14d).
import { createFileRoute } from "@tanstack/react-router";
import { VerifyScreen } from "@/features/desktops/VerifyScreen";

export const Route = createFileRoute("/desktops/$id/verify")({
  component: Page,
});

function Page() {
  const { id } = Route.useParams();
  // `key`: the fragment and the outcome of one desktop must never carry over to another.
  return <VerifyScreen key={id} id={id} />;
}
