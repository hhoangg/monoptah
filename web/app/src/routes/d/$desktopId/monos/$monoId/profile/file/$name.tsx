// `/d/:desktopId/monos/:monoId/profile/file/:name`: a document or the Soul and Memory viewer (F-76, F-79, U-09).
// Owner: WS-P (workstreams.md 3.1). `:name` is a document id; the Soul and Memory viewers are asked for with the
// `mind` search param (the Mind links of the profile), so a document that happens to be called `soul` stays a document.
import { createFileRoute } from "@tanstack/react-router";
import { MonoFileScreen } from "@/features/monos/MonoFileScreen";

export const Route = createFileRoute("/d/$desktopId/monos/$monoId/profile/file/$name")({
  validateSearch: (search: Record<string, unknown>): { mind?: "soul" | "memory" } => (search.mind === "soul" || search.mind === "memory" ? { mind: search.mind } : {}),
  component: Page,
});

function Page() {
  const { desktopId, monoId, name } = Route.useParams();
  const { mind } = Route.useSearch();
  return <MonoFileScreen key={`${desktopId}/${monoId}/${mind ?? name}`} desktopId={desktopId} monoId={monoId} name={name} mind={mind} />;
}
