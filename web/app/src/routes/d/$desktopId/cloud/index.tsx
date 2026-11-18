// Cloud instance list /d/:desktopId/cloud (U-17). Owner: WS-C (workstreams.md 3.1).
import { createFileRoute } from "@tanstack/react-router";
import { InstancesScreen } from "@/features/cloud/instances/InstancesScreen";
import { WorkspaceFrame } from "@/features/sessions/WorkspaceFrame";
import { RequireCapability } from "@/version/RequireCapability";

export const Route = createFileRoute("/d/$desktopId/cloud/")({
  component: Page,
});

function Page() {
  const { desktopId } = Route.useParams();
  return (
    <WorkspaceFrame desktopId={desktopId} title="Cloud" subtitle="Instances your sessions run on" testId="cloud-screen">
      <RequireCapability name="cloud" fallback="notice">
        <InstancesScreen desktopId={desktopId} />
      </RequireCapability>
    </WorkspaceFrame>
  );
}
