// The `files` right panel: the tree beside the transcript. Opening a file shows it in the panel with a
// back button; "Open full screen" moves to the files route.
import { Link } from "@tanstack/react-router";
import { useState } from "react";
import type { PanelProps } from "@/features/layout/panels";
import { Icon } from "@/ui/Icon";
import { DelayedSpinner } from "@/ui/DelayedSpinner";
import { FilesBrowser } from "./FilesBrowser";
import { useEntryKind } from "./useEntryKind";

export function FilesPanel({ desktopId, projectId, onClose }: PanelProps) {
  const [path, setPath] = useState("");
  const kind = useEntryKind(projectId, path);
  return (
    <div className="flex h-full min-h-0 flex-col" data-testid="files-panel">
      <div className="flex shrink-0 justify-end px-3 pt-1">
        <Link
          to="/d/$desktopId/p/$projectId/files/$"
          params={{ desktopId, projectId, _splat: path }}
          onClick={onClose}
          className="inline-flex min-h-11 items-center gap-1.5 rounded-pill px-3 text-small text-muted-text hover:bg-selection pointer-fine:min-h-9"
        >
          <Icon name="externalLink" size={16} />
          Open full screen
        </Link>
      </div>
      <div className="min-h-0 flex-1">
        {kind === "loading" ? (
          <DelayedSpinner label="Loading" className="m-6" />
        ) : (
          <FilesBrowser desktopId={desktopId} projectId={projectId} path={path} isFile={kind === "file"} layout="stack" onNavigate={setPath} />
        )}
      </div>
    </div>
  );
}
