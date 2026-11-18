// The Files surface shared by the route and the right panel: the tree with a search box, and the viewer.
//   split  tree on the left, viewer on the right (the files route from 1024px)
//   stack  tree or viewer, one at a time, with a back button (the panel, and the route below 1024px)
import { useState } from "react";
import { Icon } from "@/ui/Icon";
import { FileSearchPane } from "./FileSearchPane";
import { FileTree } from "./FileTree";
import { FileViewer } from "./FileViewer";
import { parentPath } from "./filePaths";
import { QuickOpen } from "./QuickOpen";
import { useQuickOpenHost } from "./quickOpen";

export interface FilesBrowserProps {
  desktopId: string;
  projectId: string;
  /** The open file or folder ("" for the root). */
  path: string;
  /** Whether `path` is a file (otherwise a folder, shown as an opened folder in the tree). */
  isFile: boolean;
  onNavigate: (path: string) => void;
  layout: "split" | "stack";
}

export function FilesBrowser({ desktopId, projectId, path, isFile, onNavigate, layout }: FilesBrowserProps) {
  const [search, setSearch] = useState("");
  const owner = useQuickOpenHost({ desktopId, projectId });
  const open = (next: string) => {
    setSearch("");
    onNavigate(next);
  };

  const tree = (
    <FileSearchPane projectId={projectId} search={search} onSearch={setSearch} onOpenFile={open}>
      <FileTree projectId={projectId} selected={path} onOpenFile={open} className="h-full" />
    </FileSearchPane>
  );
  const viewer = isFile ? <FileViewer projectId={projectId} path={path} onOpenPath={open} /> : null;

  return (
    <div className="flex h-full min-h-0 flex-col" data-testid="files-browser" data-layout={layout}>
      {layout === "split" ? (
        <div className="grid min-h-0 flex-1 grid-cols-[minmax(240px,320px)_minmax(0,1fr)]">
          <div className="min-h-0 border-r border-stroke">{tree}</div>
          <div className="min-h-0">{viewer ?? <p className="p-6 text-small text-muted-text">Choose a file to see it here.</p>}</div>
        </div>
      ) : viewer ? (
        <div className="flex min-h-0 flex-1 flex-col">
          <button
            type="button"
            onClick={() => onNavigate(parentPath(path))}
            className="flex min-h-11 shrink-0 items-center gap-1 border-b border-stroke px-3 text-label text-muted-text hover:bg-selection pointer-fine:min-h-9"
          >
            <Icon name="chevronRight" size={16} className="rotate-180" />
            Files
          </button>
          <div className="min-h-0 flex-1">{viewer}</div>
        </div>
      ) : (
        tree
      )}
      <QuickOpen owner={owner} desktopId={desktopId} projectId={projectId} />
    </div>
  );
}
