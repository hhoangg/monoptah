// The Files surface shared by the route and the right panel: the tree with a search box, and the viewer.
//   split  tree on the left, viewer on the right
//   stack  tree or viewer, one at a time, with a back button (the panel)
//   auto   split when this pane is at least SPLIT_MIN_WIDTH wide, else stack (the route: the pane width decides, not the window)
import { useRef, useState } from "react";
import { EmptyState } from "@/ui/EmptyState";
import { Icon } from "@/ui/Icon";
import { useElementWidth } from "@/ui/useElementWidth";
import { FileSearchPane } from "./FileSearchPane";
import { FileTree } from "./FileTree";
import { FileViewer } from "./FileViewer";
import { parentPath } from "./filePaths";
import { QuickOpen } from "./QuickOpen";
import { useQuickOpenHost } from "./quickOpenStore";

export interface FilesBrowserProps {
  desktopId: string;
  projectId: string;
  /** The open file or folder ("" for the root). */
  path: string;
  /** Whether `path` is a file (otherwise a folder, shown as an opened folder in the tree). */
  isFile: boolean;
  onNavigate: (path: string) => void;
  /** A line of the open file to jump to (from a chat mention). */
  line?: number;
  layout: "split" | "stack" | "auto";
}

/** Pane width (CSS px) from which "auto" shows the tree beside the viewer. */
export const SPLIT_MIN_WIDTH = 700;

export function FilesBrowser({ desktopId, projectId, path, isFile, onNavigate, line, layout: requested }: FilesBrowserProps) {
  const root = useRef<HTMLDivElement>(null);
  const width = useElementWidth(root);
  const layout = requested === "auto" ? (width >= SPLIT_MIN_WIDTH ? "split" : "stack") : requested;
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
  const viewer = isFile ? <FileViewer key={path} projectId={projectId} path={path} onOpenPath={open} {...(line !== undefined ? { line } : {})} /> : null;

  return (
    <div ref={root} className="flex h-full min-h-0 flex-1 flex-col" data-testid="files-browser" data-layout={layout}>
      {layout === "split" ? (
        <div className="grid min-h-0 flex-1 grid-cols-[minmax(240px,320px)_minmax(0,1fr)]">
          <div className="min-h-0 border-r border-stroke">{tree}</div>
          <div className="min-h-0">{viewer ?? <NoFileOpen />}</div>
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

function NoFileOpen() {
  return <EmptyState data-testid="no-file-open" icon="file" title="No file open" description="Pick a file or press Cmd/Ctrl P" className="h-full justify-center" />;
}
