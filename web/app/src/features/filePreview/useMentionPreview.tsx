// What a screen needs to make the file mentions of its chat clickable: `onOpenFile` for the transcript actions and
// the preview dialog to render. A text or source file of the session's own project goes to the Files route (with
// the line); everything else, and every file of a Mono chat, opens the preview. Without the `mentionedFiles`
// capability every file of the project goes to the Files route.
import { useNavigate } from "@tanstack/react-router";
import { useCallback, useState, type ReactNode } from "react";
import { useCatalog } from "@/data";
import { useCapability } from "@/version";
import { FilePreviewDialog, type PreviewTarget } from "./FilePreviewDialog";
import { routeMention } from "./resolveMention";

export interface MentionPreview {
  onOpenFile: (path: string, line?: number) => void;
  dialog: ReactNode;
}

export function useMentionPreview({ desktopId, conversationId, projectId }: { desktopId: string; conversationId: string; projectId?: string | undefined }): MentionPreview {
  const navigate = useNavigate();
  const catalog = useCatalog().data;
  const project = projectId ? catalog?.projects.find((candidate) => candidate.id === projectId) : undefined;
  const projectPath = project?.path;
  const canPreview = useCapability("mentionedFiles");
  const [target, setTarget] = useState<PreviewTarget | null>(null);
  const onOpenFile = useCallback(
    (path: string, line?: number) => {
      const route = routeMention(path, projectId && projectPath ? { path: projectPath } : undefined, canPreview);
      if (route.to === "project" && projectId) {
        void navigate({
          to: "/d/$desktopId/p/$projectId/files/$",
          params: { desktopId, projectId, _splat: route.path },
          ...(line !== undefined ? { search: { line } } : {}),
        });
        return;
      }
      setTarget({ path, ...(line !== undefined ? { line } : {}) });
    },
    [canPreview, desktopId, navigate, projectId, projectPath],
  );
  const onClose = useCallback(() => setTarget(null), []);
  return { onOpenFile, dialog: <FilePreviewDialog conversationId={conversationId} target={target} onClose={onClose} /> };
}
