// The git route /d/:desktopId/p/:projectId/git: branch, ahead/behind, upstream and PR (F-67).
import { useLinkState } from "@/link";
import { ProjectToolFrame } from "@/features/sessions/ProjectToolFrame";
import { sessionCan } from "@/ui/hooks";
import { Notice } from "@/ui/Notice";
import { GitStatusView } from "./GitStatusView";

export function GitScreen({ desktopId, projectId }: { desktopId: string; projectId: string }) {
  const capabilities = useLinkState((state) => state.capabilities);
  const allowed = sessionCan("git", { target: null, desktopCapabilities: capabilities });
  return (
    <ProjectToolFrame desktopId={desktopId} projectId={projectId} current="git" title="Git" testId="git-screen">
      {allowed ? <GitStatusView projectId={projectId} /> : <Notice className="m-4">Git is not available for this project.</Notice>}
    </ProjectToolFrame>
  );
}
