// The files route /d/:desktopId/p/:projectId/files/* (F-59 to F-62): `path` is the splat, "" is the tree root.
import { useNavigate } from "@tanstack/react-router";
import { useLinkState } from "@/link";
import { DelayedSpinner } from "@/ui/DelayedSpinner";
import { sessionCan } from "@/ui/hooks";
import { Notice } from "@/ui/Notice";
import { ProjectToolFrame } from "@/features/sessions/ProjectToolFrame";
import { FilesBrowser } from "./FilesBrowser";
import { useEntryKind } from "./useEntryKind";

export function FilesScreen({ desktopId, projectId, path, line }: { desktopId: string; projectId: string; path: string; line?: number }) {
  const navigate = useNavigate();
  const capabilities = useLinkState((state) => state.capabilities);
  const allowed = sessionCan("files", { target: null, desktopCapabilities: capabilities });
  const kind = useEntryKind(projectId, path, allowed);

  return (
    <ProjectToolFrame desktopId={desktopId} projectId={projectId} current="files" title="Files" detail={path || undefined} fill testId="files-screen">
      <div className="min-h-0 flex-1">
        {!allowed ? (
          <Notice className="m-4">Files are not available for this project.</Notice>
        ) : kind === "loading" ? (
          <DelayedSpinner label="Loading" className="m-6" />
        ) : (
          <FilesBrowser
            desktopId={desktopId}
            projectId={projectId}
            path={path}
            isFile={kind === "file"}
            {...(line !== undefined ? { line } : {})}
            layout="auto"
            onNavigate={(next) => void navigate({ to: "/d/$desktopId/p/$projectId/files/$", params: { desktopId, projectId, _splat: next } })}
          />
        )}
      </div>
    </ProjectToolFrame>
  );
}
