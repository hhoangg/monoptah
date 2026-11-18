// The Changes list and the diff of one file, shared by the route and the right panel. `path` "" is the list.
import { useEffect, useRef } from "react";
import { useLinkDesktopId } from "@/link";
import { keepDiff, keptDiff } from "./shownDiffs";
import { ChangesView } from "./ChangesView";
import { DiffView } from "./DiffView";
import { errorText } from "./gitModel";
import { useChangesQuery } from "./useGit";
import { DelayedSpinner } from "@/ui/DelayedSpinner";
import { Icon } from "@/ui/Icon";
import { Notice } from "@/ui/Notice";
import { useElementWidth } from "@/ui/useElementWidth";

/** Pane width (CSS px) from which the list and the diff sit side by side; below it the diff replaces the list. */
export const SPLIT_MIN_WIDTH = 700;

export function ChangesBrowser({ projectId, path, onNavigate }: { projectId: string; path: string; onNavigate: (path: string) => void }) {
  const desktopId = useLinkDesktopId();
  const root = useRef<HTMLDivElement>(null);
  const split = useElementWidth(root) >= SPLIT_MIN_WIDTH;
  const list = (
    <ChangesView
      projectId={projectId}
      selectedPath={path}
      onOpenFile={(file) => {
        keepDiff(desktopId, projectId, file);
        onNavigate(file.relative);
      }}
    />
  );
  return (
    <div ref={root} className="flex min-h-0 flex-1 flex-col" data-testid="changes-browser" data-layout={split ? "split" : "stack"}>
      {split ? (
        <div className="grid min-h-0 flex-1 grid-cols-[minmax(280px,360px)_minmax(0,1fr)]">
          <div className="flex min-h-0 flex-col border-r border-stroke">{list}</div>
          <div className="flex min-h-0 flex-col">
            {path === "" ? <NoDiffOpen /> : <DiffOfPath desktopId={desktopId} projectId={projectId} path={path} onBack={null} />}
          </div>
        </div>
      ) : path === "" ? (
        list
      ) : (
        <DiffOfPath desktopId={desktopId} projectId={projectId} path={path} onBack={() => onNavigate("")} />
      )}
    </div>
  );
}

function NoDiffOpen() {
  return (
    <div className="flex h-full flex-col items-center justify-center gap-2 p-6 text-center" data-testid="no-diff-open">
      <Icon name="file" size={32} className="text-faint" />
      <p className="text-title font-semibold">Choose a file</p>
      <p className="text-small text-muted-text">Pick a changed file to see its diff here</p>
    </div>
  );
}

function DiffOfPath({ desktopId, projectId, path, onBack }: { desktopId: string; projectId: string; path: string; onBack: (() => void) | null }) {
  const changes = useChangesQuery(projectId);
  const live = changes.data?.files.find((candidate) => candidate.relative === path);
  // The last diff the list showed stays on screen after a poll no longer lists it; it is remembered after render.
  const file = live ?? keptDiff(desktopId, projectId, path);
  useEffect(() => {
    if (live) keepDiff(desktopId, projectId, live);
  }, [desktopId, projectId, live]);
  return (
    <div className="flex min-h-0 flex-1 flex-col">
      {onBack ? (
        <div className="flex shrink-0 items-center px-2 pt-1">
          <button type="button" onClick={onBack} className="inline-flex min-h-11 items-center rounded-pill px-3 text-small text-muted-text hover:bg-selection pointer-fine:min-h-9">
            Back to changes
          </button>
        </div>
      ) : null}
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
