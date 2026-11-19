// The changes route /d/:desktopId/p/:projectId/changes/*: an empty splat is the list, a path the diff of that file.
import { useNavigate } from "@tanstack/react-router";
import { useLinkState } from "@/link";
import { ProjectToolFrame } from "@/features/sessions/ProjectToolFrame";
import { sessionCan } from "@/ui/hooks";
import { Notice } from "@/ui/Notice";
import { ChangesBrowser } from "./ChangesBrowser";

export function ChangesScreen({ desktopId, projectId, path }: { desktopId: string; projectId: string; path: string }) {
  const navigate = useNavigate();
  const capabilities = useLinkState((state) => state.capabilities);
  const allowed = sessionCan("changes", { target: null, desktopCapabilities: capabilities });
  return (
    <ProjectToolFrame desktopId={desktopId} projectId={projectId} current="changes" title="Changes" detail={path || undefined} fill testId="changes-screen">
      {allowed ? (
        <div className="flex min-h-0 flex-1 flex-col">
          <ChangesBrowser
            projectId={projectId}
            path={path}
            onNavigate={(next) => void navigate({ to: "/d/$desktopId/p/$projectId/changes/$", params: { desktopId, projectId, _splat: next } })}
          />
        </div>
      ) : (
        <Notice className="m-4">Changes are not available for this project.</Notice>
      )}
    </ProjectToolFrame>
  );
}
