// The `changes` and `git` right panels. Opening a file in the Changes panel shows its diff with a back button;
// "Open full screen" moves to the route.
import { Link } from "@tanstack/react-router";
import { useState } from "react";
import type { PanelProps } from "@/features/layout/panels";
import { Icon } from "@/ui/Icon";
import { ChangesBrowser } from "./ChangesBrowser";
import { GitStatusView } from "./GitStatusView";

const LINK = "inline-flex min-h-11 items-center gap-1.5 rounded-pill px-3 text-small text-muted-text hover:bg-selection pointer-fine:min-h-9";

export function ChangesPanel({ desktopId, projectId, onClose }: PanelProps) {
  const [path, setPath] = useState("");
  return (
    <div className="flex h-full min-h-0 flex-col" data-testid="changes-panel">
      <div className="flex shrink-0 justify-end px-3 pt-1">
        <Link to="/d/$desktopId/p/$projectId/changes/$" params={{ desktopId, projectId, _splat: path }} onClick={onClose} className={LINK}>
          <Icon name="externalLink" size={16} />
          Open full screen
        </Link>
      </div>
      <ChangesBrowser projectId={projectId} path={path} onNavigate={setPath} />
    </div>
  );
}

export function GitPanel({ desktopId, projectId, onClose }: PanelProps) {
  return (
    <div className="flex h-full min-h-0 flex-col overflow-y-auto" data-testid="git-panel">
      <div className="flex shrink-0 justify-end px-3 pt-1">
        <Link to="/d/$desktopId/p/$projectId/git" params={{ desktopId, projectId }} onClick={onClose} className={LINK}>
          <Icon name="externalLink" size={16} />
          Open full screen
        </Link>
      </div>
      <GitStatusView projectId={projectId} />
    </div>
  );
}
