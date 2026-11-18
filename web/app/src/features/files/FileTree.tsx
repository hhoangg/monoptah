// F-59: the file tree. Folders load lazily when opened (`project.files`), ignored entries are dimmed,
// the first four root folders are prefetched, and a refresh button re-reads what is loaded (the web has no pull).
import { useQueryClient } from "@tanstack/react-query";
import { useEffect, useState } from "react";
import { linkQueryOptions, useLinkQuery } from "@/data";
import { useLinkDesktopId } from "@/link";
import { cn } from "@/lib/utils";
import { DelayedSpinner } from "@/ui/DelayedSpinner";
import { Icon } from "@/ui/Icon";
import { Notice } from "@/ui/Notice";
import { FileTypeIcon } from "./FileTypeIcon";
import { ancestorsOf, folderParams } from "./filePaths";

export const PREFETCH_FOLDERS = 4;

interface Entry {
  name: string;
  isDir: boolean;
  ignored: boolean;
}

/** Folders first, then files, each by name. */
export function sortEntries<T extends Entry>(entries: readonly T[]): T[] {
  return [...entries].sort((a, b) => Number(b.isDir) - Number(a.isDir) || a.name.localeCompare(b.name));
}

const rowClass =
  "flex min-h-11 w-full items-center gap-2 rounded-row px-2 text-left text-label hover:bg-selection pointer-fine:min-h-8";

interface TreeContext {
  projectId: string;
  expanded: ReadonlySet<string>;
  selected: string;
  onToggle: (path: string) => void;
  onOpenFile: (path: string) => void;
}

function Folder({ path, depth, context }: { path: string; depth: number; context: TreeContext }) {
  const query = useLinkQuery("project.files", folderParams(context.projectId, path));
  if (query.isError) {
    return (
      <li role="none" style={{ paddingLeft: depth * 16 + 8 }} className="py-1 text-small text-deletions">
        Could not load this folder.
      </li>
    );
  }
  if (!query.data) {
    return (
      <li role="none" style={{ paddingLeft: depth * 16 + 8 }}>
        <DelayedSpinner label="Loading folder" className="my-1" />
      </li>
    );
  }
  const entries = sortEntries(query.data.entries);
  if (entries.length === 0) {
    return (
      <li role="none" style={{ paddingLeft: depth * 16 + 8 }} className="py-1 text-small text-muted-text">
        Empty folder
      </li>
    );
  }
  return (
    <>
      {entries.map((entry) => {
        const full = path ? `${path}/${entry.name}` : entry.name;
        const open = entry.isDir && context.expanded.has(full);
        return (
          <li key={entry.name} role="none">
            <button
              type="button"
              role="treeitem"
              aria-level={depth + 1}
              aria-expanded={entry.isDir ? open : undefined}
              aria-selected={!entry.isDir && context.selected === full}
              data-ignored={entry.ignored || undefined}
              data-kind={entry.isDir ? "folder" : "file"}
              onClick={() => (entry.isDir ? context.onToggle(full) : context.onOpenFile(full))}
              className={cn(rowClass, entry.ignored && "opacity-50", !entry.isDir && context.selected === full && "bg-selection")}
              style={{ paddingLeft: depth * 16 + 8 }}
            >
              {entry.isDir ? <Icon name="chevronRight" size={14} className={cn("shrink-0 text-muted-text transition-transform motion-reduce:transition-none", open && "rotate-90")} /> : <span className="w-3.5 shrink-0" />}
              <FileTypeIcon name={entry.name} isDir={entry.isDir} isOpen={open} />
              <span className="min-w-0 flex-1 truncate">{entry.name}</span>
            </button>
            {open ? (
              <ul role="group">
                <Folder path={full} depth={depth + 1} context={context} />
              </ul>
            ) : null}
          </li>
        );
      })}
    </>
  );
}

export interface FileTreeProps {
  projectId: string;
  /** The open file or folder: its folders are opened so it shows. */
  selected: string;
  onOpenFile: (path: string) => void;
  /** Folders to open at first (the ancestors of `selected` are always opened). */
  className?: string;
}

export function FileTree({ projectId, selected, onOpenFile, className }: FileTreeProps) {
  const desktopId = useLinkDesktopId();
  const queryClient = useQueryClient();
  const root = useLinkQuery("project.files", folderParams(projectId, ""));
  const [expanded, setExpanded] = useState<ReadonlySet<string>>(() => new Set(ancestorsOf(selected)));

  // Reveal the selected path when it changes (a link, quick open or a search hit): state adjusted while rendering.
  const [revealed, setRevealed] = useState(selected);
  if (revealed !== selected) {
    setRevealed(selected);
    const parts = ancestorsOf(selected);
    if (parts.some((part) => !expanded.has(part))) setExpanded(new Set([...expanded, ...parts]));
  }

  // Prefetch the first folders so opening one is instant.
  const folders = root.data ? sortEntries(root.data.entries).filter((entry) => entry.isDir).slice(0, PREFETCH_FOLDERS) : [];
  const prefetchKey = folders.map((entry) => entry.name).join("\0");
  useEffect(() => {
    for (const name of prefetchKey ? prefetchKey.split("\0") : []) {
      void queryClient.prefetchQuery(linkQueryOptions(desktopId, "project.files", folderParams(projectId, name)));
    }
  }, [queryClient, desktopId, projectId, prefetchKey]);

  const refresh = () =>
    void queryClient.invalidateQueries({
      predicate: ({ queryKey }) => {
        const params = queryKey[3] as { projectId?: string } | undefined;
        return queryKey[0] === "link" && queryKey[1] === desktopId && (queryKey[2] === "project.files" || queryKey[2] === "project.file") && params?.projectId === projectId;
      },
    });

  const context: TreeContext = {
    projectId,
    expanded,
    selected,
    onToggle: (path) =>
      setExpanded((current) => {
        const next = new Set(current);
        if (!next.delete(path)) next.add(path);
        return next;
      }),
    onOpenFile,
  };

  return (
    <div className={cn("flex min-h-0 flex-col", className)} data-testid="file-tree">
      <div className="flex items-center justify-between gap-2 px-3 pt-1">
        <h2 className="text-meta font-medium text-muted-text">Files</h2>
        <button
          type="button"
          aria-label="Refresh files"
          onClick={refresh}
          className="inline-flex size-11 items-center justify-center rounded-pill text-muted-text hover:bg-selection pointer-fine:size-9"
        >
          <Icon name="refresh" size={18} />
        </button>
      </div>
      <div className="min-h-0 flex-1 overflow-y-auto px-1 pb-3">
        {root.isError ? (
          <Notice variant="error" className="m-2">Could not load the files of this project.</Notice>
        ) : (
          <ul role="tree" aria-label="Project files">
            <Folder path="" depth={0} context={context} />
          </ul>
        )}
      </div>
    </div>
  );
}
