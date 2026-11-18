// The Changes list and the diff of one file, shared by the route and the right panel. `path` "" is the list.
import { useEffect } from "react";
import { useLinkDesktopId } from "@/link";
import { keepDiff, keptDiff } from "./shownDiffs";
import { ChangesView } from "./ChangesView";
import { DiffView } from "./DiffView";
import { errorText } from "./gitModel";
import { useChangesQuery } from "./useGit";
import { DelayedSpinner } from "@/ui/DelayedSpinner";
import { Notice } from "@/ui/Notice";

export function ChangesBrowser({ projectId, path, onNavigate }: { projectId: string; path: string; onNavigate: (path: string) => void }) {
  const desktopId = useLinkDesktopId();
  if (path === "") {
    return (
      <div className="min-h-0 flex-1 overflow-y-auto">
        <ChangesView
          projectId={projectId}
          onOpenFile={(file) => {
            keepDiff(desktopId, projectId, file);
            onNavigate(file.relative);
          }}
        />
      </div>
    );
  }
  return <DiffOfPath desktopId={desktopId} projectId={projectId} path={path} onBack={() => onNavigate("")} />;
}

function DiffOfPath({ desktopId, projectId, path, onBack }: { desktopId: string; projectId: string; path: string; onBack: () => void }) {
  const changes = useChangesQuery(projectId);
  const live = changes.data?.files.find((candidate) => candidate.relative === path);
  // The last diff the list showed stays on screen after a poll no longer lists it; it is remembered after render.
  const file = live ?? keptDiff(desktopId, projectId, path);
  useEffect(() => {
    if (live) keepDiff(desktopId, projectId, live);
  }, [desktopId, projectId, live]);
  return (
    <div className="flex min-h-0 flex-1 flex-col">
      <div className="flex shrink-0 items-center px-2 pt-1">
        <button type="button" onClick={onBack} className="inline-flex min-h-11 items-center rounded-pill px-3 text-small text-muted-text hover:bg-selection pointer-fine:min-h-9">
          Back to changes
        </button>
      </div>
      {file ? (
        <DiffView key={file.relative} file={file} />
      ) : changes.isError ? (
        <Notice variant="error" className="m-4">
          {errorText(changes.error)}
        </Notice>
      ) : !changes.data ? (
        <DelayedSpinner label="Loading diff" className="m-6" />
      ) : (
        <Notice className="m-4" title="No longer changed">
          This file has no uncommitted changes now.
        </Notice>
      )}
    </div>
  );
}
